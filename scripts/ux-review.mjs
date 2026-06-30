#!/usr/bin/env bun
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const requireFromWeb = createRequire(new URL("../web/package.json", import.meta.url));

let chromium;
try {
  ({ chromium } = requireFromWeb("@playwright/test"));
} catch (error) {
  console.error("Could not load @playwright/test from web/node_modules.");
  console.error("Run `make web-install` first, then rerun `make ux-review`.");
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

const args = parseArgs(process.argv.slice(2));
const baseUrl = stripTrailingSlash(
  args["base-url"] ?? process.env.UX_REVIEW_BASE_URL ?? "http://localhost:5173",
);
const symbolList = (args.symbols ?? process.env.UX_REVIEW_SYMBOLS ?? "OKTA,NVDA,CRDO")
  .split(",")
  .map((symbol) => symbol.trim().toUpperCase())
  .filter(Boolean);
const now = new Date();
const timestamp = now.toISOString().replace(/[:.]/g, "-");
const outRoot = path.resolve(
  args.out ?? process.env.UX_REVIEW_OUT ?? path.join(repoRoot, ".runtime", "ux-review"),
);
const outDir = path.join(outRoot, timestamp);
const headless = !args.headful;

const viewports = [
  { name: "desktop", width: 1440, height: 1000 },
  { name: "narrow", width: 390, height: 844 },
];

const routes = [
  { name: "workspace", path: "/" },
  { name: "automation", path: "/automation" },
  ...symbolList.flatMap((symbol) => [
    { name: `symbol-${symbol.toLowerCase()}`, path: `/symbol/${encodeURIComponent(symbol)}` },
    {
      name: `automation-${symbol.toLowerCase()}`,
      path: `/automation/${encodeURIComponent(symbol)}`,
    },
  ]),
  { name: "journal", path: "/journal" },
  { name: `journal-${todayIso()}`, path: `/journal/${todayIso()}` },
];

await mkdir(outDir, { recursive: true });

const browser = await chromium.launch({ headless });
const captures = [];
const reviewPacketAttempts = [];

try {
  for (const viewport of viewports) {
    const page = await browser.newPage({
      viewport: { width: viewport.width, height: viewport.height },
    });
    page.setDefaultTimeout(7_000);

    for (const route of routes) {
      captures.push(await captureRoute(page, viewport, route));
    }

    reviewPacketAttempts.push(await captureFirstReviewPacket(page, viewport));
    await page.close();
  }
} finally {
  await browser.close();
}

await writeReport({ captures, reviewPacketAttempts });
console.log(`UX review artifacts written to ${outDir}`);

function parseArgs(argv) {
  const parsed = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) continue;
    const key = arg.slice(2);
    if (key === "headful") {
      parsed.headful = true;
      continue;
    }
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      parsed[key] = "true";
      continue;
    }
    parsed[key] = next;
    i += 1;
  }
  return parsed;
}

function stripTrailingSlash(value) {
  return value.replace(/\/+$/, "");
}

function todayIso() {
  return new Date().toISOString().slice(0, 10);
}

function safeName(value) {
  return value.toLowerCase().replace(/[^a-z0-9-]+/g, "-").replace(/^-|-$/g, "");
}

async function captureRoute(page, viewport, route) {
  const name = `${viewport.name}-${safeName(route.name)}`;
  const url = `${baseUrl}${route.path}`;
  const screenshot = `${name}.png`;
  const snapshotFile = `${name}.json`;
  const result = {
    name: route.name,
    viewport: viewport.name,
    path: route.path,
    url,
    screenshot,
    snapshot: snapshotFile,
    status: "ok",
    error: null,
  };

  try {
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30_000 });
    await page.waitForLoadState("networkidle", { timeout: 4_000 }).catch(() => {});
    await page.screenshot({ path: path.join(outDir, screenshot), fullPage: true });
    const snapshot = await collectPageSnapshot(page);
    await writeFile(path.join(outDir, snapshotFile), `${JSON.stringify(snapshot, null, 2)}\n`);
  } catch (error) {
    result.status = "error";
    result.error = error instanceof Error ? error.message : String(error);
    await writeFile(path.join(outDir, snapshotFile), `${JSON.stringify(result, null, 2)}\n`);
  }

  return result;
}

async function captureFirstReviewPacket(page, viewport) {
  const result = {
    viewport: viewport.name,
    url: `${baseUrl}/`,
    clicked: false,
    screenshot: `${viewport.name}-review-packet-attempt.png`,
    snapshot: `${viewport.name}-review-packet-attempt.json`,
    error: null,
  };

  try {
    await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded", timeout: 30_000 });
    await page.waitForLoadState("networkidle", { timeout: 4_000 }).catch(() => {});
    const candidates = [
      page.getByRole("button", { name: /open review packet/i }),
      page.getByRole("button", { name: /review packet/i }),
      page.getByRole("button", { name: /thesis changed|actionable|ready for review/i }),
    ];

    for (const locator of candidates) {
      const count = await locator.count().catch(() => 0);
      if (count < 1) continue;
      await locator.first().click({ timeout: 3_000 });
      result.clicked = true;
      await waitForReviewPacketContent(page);
      break;
    }

    await page.screenshot({ path: path.join(outDir, result.screenshot), fullPage: true });
    const snapshot = await collectPageSnapshot(page);
    await writeFile(path.join(outDir, result.snapshot), `${JSON.stringify(snapshot, null, 2)}\n`);
  } catch (error) {
    result.error = error instanceof Error ? error.message : String(error);
    await writeFile(path.join(outDir, result.snapshot), `${JSON.stringify(result, null, 2)}\n`);
  }

  return result;
}

async function waitForReviewPacketContent(page) {
  await page.waitForFunction(
    () => {
      const packet = document.querySelector("[data-testid='review-packet']");
      if (!packet) return false;
      const text = packet.textContent ?? "";
      return text.trim().length > 80 && !/loading review packet/i.test(text);
    },
    null,
    { timeout: 10_000 },
  ).catch(() => {});
}

async function collectPageSnapshot(page) {
  return page.evaluate(() => {
    const visibleText = (element) => (element.textContent ?? "").replace(/\s+/g, " ").trim();
    const isVisible = (element) => {
      const style = window.getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
    };
    const selectorText = (selector, limit = 80) => Array.from(document.querySelectorAll(selector))
      .filter(isVisible)
      .map((element) => visibleText(element))
      .filter(Boolean)
      .slice(0, limit);
    const controls = Array.from(
      document.querySelectorAll("button, a, input, select, textarea, [role='button'], [role='link']"),
    )
      .filter(isVisible)
      .map((element) => ({
        tag: element.tagName.toLowerCase(),
        role: element.getAttribute("role"),
        type: element.getAttribute("type"),
        text: visibleText(element) || element.getAttribute("aria-label") || element.getAttribute("placeholder") || "",
        disabled: Boolean(element.disabled) || element.getAttribute("aria-disabled") === "true",
        testid: element.getAttribute("data-testid"),
      }))
      .filter((item) => item.text || item.testid)
      .slice(0, 140);

    return {
      title: document.title,
      url: window.location.href,
      viewport: {
        width: window.innerWidth,
        height: window.innerHeight,
      },
      headings: selectorText("h1, h2, h3, [role='heading']", 80),
      strongText: selectorText("strong", 80),
      controls,
      testids: Array.from(document.querySelectorAll("[data-testid]"))
        .filter(isVisible)
        .map((element) => element.getAttribute("data-testid"))
        .filter(Boolean)
        .slice(0, 120),
      bodyText: (document.body.innerText ?? "").replace(/\s+/g, " ").trim().slice(0, 24_000),
    };
  });
}

async function writeReport({ captures, reviewPacketAttempts }) {
  const lines = [
    "# UX Review Capture",
    "",
    `Generated: ${now.toISOString()}`,
    `Base URL: ${baseUrl}`,
    `Symbols: ${symbolList.join(", ") || "(none)"}`,
    "",
    "Use this capture with `docs/UX_REVIEW.md`. The screenshots and JSON files are evidence for a human workflow review; they are not a pass/fail test.",
    "",
    "## Captured Routes",
    "",
    "| Viewport | Route | Status | Screenshot | Snapshot |",
    "| --- | --- | --- | --- | --- |",
    ...captures.map((capture) => (
      `| ${capture.viewport} | \`${capture.path}\` | ${capture.status}${capture.error ? `: ${escapeTable(capture.error)}` : ""} | ${capture.status === "ok" ? `[png](./${capture.screenshot})` : ""} | [json](./${capture.snapshot}) |`
    )),
    "",
    "## Review Packet Attempt",
    "",
    "| Viewport | Clicked entry | Screenshot | Snapshot | Error |",
    "| --- | --- | --- | --- | --- |",
    ...reviewPacketAttempts.map((attempt) => (
      `| ${attempt.viewport} | ${attempt.clicked ? "yes" : "no"} | [png](./${attempt.screenshot}) | [json](./${attempt.snapshot}) | ${attempt.error ? escapeTable(attempt.error) : ""} |`
    )),
    "",
    "## Findings",
    "",
    "Fill this out after reviewing screenshots and interacting with Chromium.",
    "",
    "### Finding Template",
    "",
    "- Severity:",
    "- Screen/route:",
    "- Reproduction path:",
    "- Expected operator decision:",
    "- Actual UX:",
    "- Evidence:",
    "- Recommended fix:",
    "",
    "## Workflow Verdicts",
    "",
    "- Attention queue:",
    "- Symbol workspace:",
    "- Review packet:",
    "- Autonomous cockpit:",
    "- Decisions/positions:",
    "- Daily trade desk/journal:",
    "",
  ];

  await writeFile(path.join(outDir, "report.md"), `${lines.join("\n")}\n`);
}

function escapeTable(value) {
  return value.replace(/\|/g, "\\|").replace(/\n/g, " ");
}
