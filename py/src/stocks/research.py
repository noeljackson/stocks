"""Product/theme web research retrieval.

The first provider is GDELT Doc 2.0 because it requires no API key and gives us
a real web/news retrieval pass before the thesis engine claims "no public data".
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import hashlib
import json
import logging
import os
import re
from dataclasses import dataclass
from email.utils import parsedate_to_datetime
from urllib.parse import parse_qs, unquote, urljoin, urlparse
from xml.etree import ElementTree

import asyncpg
import httpx

from . import config

log = logging.getLogger("research")

SOURCE = "web_research"

STATIC_PRODUCT_TERMS: dict[str, list[str]] = {
    "AMD": ["MI325X", "MI355X", "MI400", "ROCm", "vLLM"],
    "NVDA": ["Blackwell", "GB200", "H200", "CUDA", "NVL72"],
    "MU": ["HBM3E", "HBM4", "advanced packaging"],
    "DELL": ["AI server", "PowerEdge", "NVIDIA GB200", "enterprise AI"],
    "CRWV": ["GB200", "NVIDIA cloud", "AI data center", "hyperscaler contract"],
    "LITE": ["800G", "1.6T optical", "datacenter transceiver", "AI cluster optics"],
    "TSM": ["CoWoS", "2nm", "advanced packaging", "AI accelerator"],
}

PRODUCT_TOKEN_RE = re.compile(
    r"\b(?:[A-Z]{2,}\d{2,}[A-Z0-9]*|MI\d{3,4}X|GB\d{3,}|HBM\d?[A-Z]*|CoWoS|ROCm|vLLM)\b"
)


@dataclass(frozen=True)
class SearchResult:
    title: str
    url: str
    publisher: str | None
    published_at: dt.datetime | None
    summary: str | None
    source_type: str
    credibility: str
    source_ref: dict


@dataclass(frozen=True)
class ResearchRelevance:
    accepted: bool
    score: float
    reasons: tuple[str, ...]
    matched_terms: tuple[str, ...]
    rejected_reason: str | None = None
    unrelated_tickers: tuple[str, ...] = ()

    def source_ref(self) -> dict:
        return {
            "accepted": self.accepted,
            "score": self.score,
            "reasons": list(self.reasons),
            "matched_terms": list(self.matched_terms),
            "rejected_reason": self.rejected_reason,
            "unrelated_tickers": list(self.unrelated_tickers),
        }


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError:
        log.warning("invalid %s=%r; using %d", name, raw, default)
        return default


def _truncate(value: str | None, limit: int = 500) -> str | None:
    if value is None:
        return None
    return value[:limit]


def _parse_time(value: str | None) -> dt.datetime | None:
    if not value:
        return None
    for fmt in ("%Y%m%d%H%M%S", "%Y-%m-%dT%H:%M:%SZ", "%Y-%m-%d"):
        try:
            parsed = dt.datetime.strptime(value, fmt)
            return parsed.replace(tzinfo=dt.UTC)
        except ValueError:
            continue
    try:
        parsed = parsedate_to_datetime(value)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=dt.UTC)
        return parsed.astimezone(dt.UTC)
    except (TypeError, ValueError):
        pass
    return None


def _credibility(url: str, publisher: str | None) -> str:
    host = urlparse(url).netloc.lower()
    label = (publisher or "").lower()
    primary_hosts = (
        "amd.com",
        "nvidia.com",
        "dell.com",
        "micron.com",
        "tsmc.com",
        "sec.gov",
        "opencompute.org",
        "mlcommons.org",
        "github.com",
    )
    credible_hosts = (
        "reuters.com",
        "bloomberg.com",
        "wsj.com",
        "ft.com",
        "theinformation.com",
        "semianalysis.com",
        "servethehome.com",
        "nextplatform.com",
    )
    if any(host.endswith(h) for h in primary_hosts):
        return "primary"
    if any(host.endswith(h) for h in credible_hosts) or any(
        name in label for name in ("reuters", "bloomberg", "financial times")
    ):
        return "credible_media"
    if any(term in host for term in ("semi", "hpc", "datacenter", "tech", "servethehome")):
        return "industry"
    return "unknown"


def _evidence_strength(credibility: str) -> float:
    return {
        "primary": 0.9,
        "credible_media": 0.75,
        "industry": 0.6,
        "unknown": 0.4,
    }.get(credibility, 0.4)


def _canonical_url(url: str) -> str:
    parsed = urlparse(url)
    if parsed.netloc.lower().endswith("bing.com") and parsed.path.endswith("/news/apiclick.aspx"):
        target = parse_qs(parsed.query).get("url", [None])[0]
        if target:
            return unquote(target)
    return url


def _extract_terms(context: dict | None) -> list[str]:
    if not context:
        return []
    text = json.dumps(context, default=str)
    seen: set[str] = set()
    out: list[str] = []
    for match in PRODUCT_TOKEN_RE.finditer(text):
        term = match.group(0)
        if term not in seen:
            seen.add(term)
            out.append(term)
    return out[:8]


async def _company_profile(pool: asyncpg.Pool, symbol: str) -> dict:
    row = await pool.fetchrow(
        """SELECT dp.company_name, dp.industry, dp.sector, t.cluster_id
             FROM ticker t
        LEFT JOIN discovery_pool dp ON dp.symbol = t.symbol
            WHERE t.symbol = $1""",
        symbol,
    )
    if row is None:
        return {}
    return {k: row[k] for k in row.keys()}


def build_queries(
    symbol: str,
    profile: dict,
    context: dict | None,
    max_queries: int = 6,
    extra_queries: list[str] | None = None,
) -> list[str]:
    company = (
        (profile.get("company_name") or symbol)
        .replace(", Inc.", "")
        .replace(" Corporation", "")
    )
    company = " ".join(COMPANY_SUFFIX_RE.sub(" ", company.replace(",", " ")).split()) or symbol
    industry = profile.get("industry")
    terms = STATIC_PRODUCT_TERMS.get(symbol.upper(), []) + _extract_terms(context)

    raw: list[str] = []
    company_aliases = _company_aliases(symbol, profile)
    for query in extra_queries or []:
        query = " ".join(str(query).split())
        if not query:
            continue
        normalized = _normalized_phrase(query)
        if (
            not _symbol_in_text(symbol, query)
            and not any(alias and alias in normalized for alias in company_aliases)
        ):
            query = f"{company} {query}"
        raw.append(query)
    for term in terms:
        raw.append(f"{company} {term} deployment benchmark adoption")
        raw.append(f"{symbol} {term} vs competitor customer production")
    if industry:
        raw.append(f"{company} {industry} latest demand pricing margins customers")
        raw.append(f"{symbol} {industry} supply demand estimates revisions catalyst")
    raw.append(f"{company} latest product customer deployment benchmark")

    seen: set[str] = set()
    queries: list[str] = []
    for query in raw:
        normalized = " ".join(query.split())
        key = normalized.lower()
        if key in seen:
            continue
        seen.add(key)
        queries.append(normalized)
        if len(queries) >= max_queries:
            break
    return queries


COMPANY_SUFFIX_RE = re.compile(
    r"\b(?:"
    r"inc|incorporated|corp|corporation|company|co|ltd|limited|plc|"
    r"holdings|holding|group|llc|lp|sa|ag|nv|se|ordinary|common|stock"
    r")\b\.?",
    re.IGNORECASE,
)
NON_ALNUM_RE = re.compile(r"[^a-z0-9]+")
SPECIFIC_PRODUCT_WORDS = {
    "blackwell",
    "cuda",
    "cowos",
    "poweredge",
    "rocm",
    "vllm",
}
COMMON_TICKER_COLLISIONS = {
    "AI",
    "API",
    "CEO",
    "CFO",
    "EPS",
    "ETF",
    "GPU",
    "HBM",
    "Q1",
    "Q2",
    "Q3",
    "Q4",
    "SEC",
    "USA",
}


def _normalized_phrase(value: str | None) -> str:
    if not value:
        return ""
    return NON_ALNUM_RE.sub(" ", value.lower()).strip()


def _phrase_in_text(phrase: str, normalized_text: str) -> bool:
    normalized = _normalized_phrase(phrase)
    if not normalized:
        return False
    return f" {normalized} " in f" {normalized_text} "


def _company_aliases(symbol: str, profile: dict | None) -> list[str]:
    raw_company = str((profile or {}).get("company_name") or "").strip()
    aliases: list[str] = []
    seen: set[str] = set()

    def add(value: str | None) -> None:
        normalized = _normalized_phrase(value)
        if len(normalized) < 3 or normalized in seen:
            return
        seen.add(normalized)
        aliases.append(normalized)

    if raw_company:
        add(raw_company.split(",", 1)[0])
        cleaned = COMPANY_SUFFIX_RE.sub(" ", raw_company.replace(",", " "))
        cleaned = " ".join(cleaned.split())
        add(cleaned)

    base_symbol = symbol.split(".", 1)[0]
    if not base_symbol.isdigit():
        add(base_symbol)
    return aliases


def _symbol_in_text(symbol: str, raw_text: str) -> bool:
    base_symbol = symbol.split(".", 1)[0].upper()
    if not base_symbol:
        return False
    flags = 0 if len(base_symbol) <= 2 else re.IGNORECASE
    return bool(
        re.search(
            rf"(?<![A-Za-z0-9]){re.escape(base_symbol)}(?![A-Za-z0-9])",
            raw_text,
            flags,
        )
    )


def _result_text(result: SearchResult) -> tuple[str, str]:
    parsed = urlparse(result.url)
    raw = " ".join(
        part
        for part in (
            result.title,
            result.summary or "",
            result.publisher or "",
            parsed.netloc,
            parsed.path.replace("-", " ").replace("_", " "),
        )
        if part
    )
    return raw, _normalized_phrase(raw)


def _product_terms(symbol: str, query: str, tags: list[str]) -> list[str]:
    out: list[str] = []
    seen: set[str] = set()

    def add(term: str | None) -> None:
        if not term:
            return
        cleaned = " ".join(str(term).split())
        key = cleaned.lower()
        if not cleaned or key == symbol.lower() or key in seen:
            return
        seen.add(key)
        out.append(cleaned)

    for term in STATIC_PRODUCT_TERMS.get(symbol.upper(), []):
        add(term)
    for term in tags:
        add(term)
    for match in PRODUCT_TOKEN_RE.finditer(query):
        add(match.group(0))
    return out


def _specific_product_term(term: str) -> bool:
    lowered = term.lower()
    return (
        any(ch.isdigit() for ch in term)
        or lowered in SPECIFIC_PRODUCT_WORDS
        or bool(PRODUCT_TOKEN_RE.search(term))
    )


def _unrelated_tickers(raw_text: str, symbol: str) -> tuple[str, ...]:
    base_symbol = symbol.split(".", 1)[0].upper()
    candidates = set(re.findall(r"\(([A-Z]{1,6})\)", raw_text))
    candidates.update(re.findall(r"\b([A-Z]{2,6})\s+Q[1-4]\s+20\d{2}\s+Earnings", raw_text))
    return tuple(sorted(
        candidate
        for candidate in candidates
        if candidate != base_symbol and candidate not in COMMON_TICKER_COLLISIONS
    ))


def research_relevance(
    symbol: str,
    profile: dict | None,
    query: str,
    result: SearchResult,
    tags: list[str],
) -> ResearchRelevance:
    raw_text, normalized_text = _result_text(result)
    score = 0.0
    reasons: list[str] = []
    matched_terms: list[str] = []

    if _symbol_in_text(symbol, raw_text):
        score = max(score, 0.9)
        reasons.append("symbol")
        matched_terms.append(symbol.split(".", 1)[0].upper())

    for alias in _company_aliases(symbol, profile):
        if _phrase_in_text(alias, normalized_text):
            score = max(score, 0.85)
            reasons.append("company_alias")
            matched_terms.append(alias)
            break

    for term in _product_terms(symbol, query, tags):
        if not _phrase_in_text(term, normalized_text):
            continue
        matched_terms.append(term)
        if _specific_product_term(term):
            score = max(score, 0.75)
            reasons.append("specific_product_term")
        else:
            score = max(score, 0.45)
            reasons.append("generic_theme_term")

    unique_reasons = tuple(dict.fromkeys(reasons))
    unique_terms = tuple(dict.fromkeys(matched_terms))
    unrelated = _unrelated_tickers(raw_text, symbol)
    accepted = score >= 0.7
    rejected_reason = None
    if not accepted:
        if unrelated:
            rejected_reason = "dominant_unrelated_ticker"
        elif unique_terms:
            rejected_reason = "generic_theme_without_symbol_or_company"
        else:
            rejected_reason = "no_symbol_company_or_product_match"

    return ResearchRelevance(
        accepted=accepted,
        score=round(score, 3),
        reasons=unique_reasons,
        matched_terms=unique_terms,
        rejected_reason=rejected_reason,
        unrelated_tickers=unrelated,
    )


class GdeltProvider:
    name = "gdelt_doc"

    def __init__(self, *, timeout_seconds: float = 15.0) -> None:
        self._client = httpx.AsyncClient(
            timeout=timeout_seconds,
            headers={"user-agent": "stocks-research/0.1 (+https://github.com/noeljackson/stocks)"},
        )

    async def close(self) -> None:
        await self._client.aclose()

    async def search(self, query: str, *, max_results: int) -> list[SearchResult]:
        resp = await self._client.get(
            "https://api.gdeltproject.org/api/v2/doc/doc",
            params={
                "query": query,
                "mode": "ArtList",
                "format": "json",
                "maxrecords": max_results,
                "sort": "HybridRel",
                "timespan": "90d",
            },
        )
        resp.raise_for_status()
        payload = resp.json()
        articles = payload.get("articles") or []
        out: list[SearchResult] = []
        for item in articles:
            url = item.get("url")
            title = item.get("title")
            if not url or not title:
                continue
            publisher = item.get("domain") or item.get("sourcecountry")
            out.append(
                SearchResult(
                    title=title,
                    url=url,
                    publisher=publisher,
                    published_at=_parse_time(item.get("seendate")),
                    summary=item.get("socialimage"),
                    source_type="news_search",
                    credibility=_credibility(url, publisher),
                    source_ref={"provider_payload": item},
                )
            )
        return out


class BingNewsProvider:
    name = "bing_news_rss"

    def __init__(self, *, timeout_seconds: float = 15.0) -> None:
        self._client = httpx.AsyncClient(
            timeout=timeout_seconds,
            headers={"user-agent": "stocks-research/0.1 (+https://github.com/noeljackson/stocks)"},
        )

    async def close(self) -> None:
        await self._client.aclose()

    async def search(self, query: str, *, max_results: int) -> list[SearchResult]:
        resp = await self._client.get(
            "https://www.bing.com/news/search",
            params={"q": query, "format": "rss"},
        )
        resp.raise_for_status()
        root = ElementTree.fromstring(resp.text)
        out: list[SearchResult] = []
        for item in root.findall("./channel/item"):
            title = item.findtext("title")
            raw_url = item.findtext("link")
            url = _canonical_url(raw_url) if raw_url else None
            if not title or not url:
                continue
            publisher = item.findtext("source")
            publisher = publisher or urlparse(url).netloc
            published_at = None
            raw_date = item.findtext("pubDate")
            if raw_date:
                try:
                    published_at = parsedate_to_datetime(raw_date)
                    if published_at.tzinfo is None:
                        published_at = published_at.replace(tzinfo=dt.UTC)
                except (TypeError, ValueError):
                    published_at = None
            out.append(
                SearchResult(
                    title=title,
                    url=url,
                    publisher=publisher or urlparse(url).netloc,
                    published_at=published_at,
                    summary=item.findtext("description"),
                    source_type="news_search",
                    credibility=_credibility(url, publisher),
                    source_ref={"provider": self.name},
                )
            )
            if len(out) >= max_results:
                break
        return out


class FirecrawlProvider:
    name = "firecrawl"

    def __init__(
        self,
        *,
        base_url: str | None = None,
        api_key: str | None = None,
        search_path: str | None = None,
        timeout_seconds: float = 30.0,
        transport: httpx.AsyncBaseTransport | None = None,
    ) -> None:
        self.base_url = (base_url or os.getenv("FIRECRAWL_BASE_URL") or "http://localhost:3002").rstrip("/")
        self.search_path = search_path or os.getenv("FIRECRAWL_SEARCH_PATH", "/v2/search")
        headers = {
            "user-agent": "stocks-research/0.1 (+https://github.com/noeljackson/stocks)",
            "content-type": "application/json",
        }
        api_key = api_key if api_key is not None else os.getenv("FIRECRAWL_API_KEY")
        if api_key:
            headers["authorization"] = f"Bearer {api_key}"
        self._client = httpx.AsyncClient(
            timeout=timeout_seconds,
            headers=headers,
            transport=transport,
        )

    async def close(self) -> None:
        await self._client.aclose()

    async def search(self, query: str, *, max_results: int) -> list[SearchResult]:
        resp = await self._client.post(
            urljoin(f"{self.base_url}/", self.search_path.lstrip("/")),
            json={
                "query": query,
                "limit": max_results,
                "sources": ["web"],
                "country": os.getenv("FIRECRAWL_SEARCH_COUNTRY", "US"),
                "tbs": os.getenv("FIRECRAWL_SEARCH_TBS", "qdr:m"),
                "ignoreInvalidURLs": True,
            },
        )
        resp.raise_for_status()
        payload = resp.json()
        return self._parse_results(payload, max_results=max_results)

    def _parse_results(self, payload: dict, *, max_results: int) -> list[SearchResult]:
        data = payload.get("data") or {}
        buckets: list[tuple[str, dict]] = []
        if isinstance(data, list):
            buckets.extend(("web", item) for item in data if isinstance(item, dict))
        elif isinstance(data, dict):
            for source_type in ("web", "news"):
                rows = data.get(source_type) or []
                if isinstance(rows, list):
                    buckets.extend((source_type, item) for item in rows if isinstance(item, dict))

        out: list[SearchResult] = []
        seen_urls: set[str] = set()
        for bucket, item in buckets:
            metadata = item.get("metadata") if isinstance(item.get("metadata"), dict) else {}
            url = item.get("url") or metadata.get("sourceURL") or metadata.get("url")
            title = item.get("title") or metadata.get("title")
            if not url or not title:
                continue
            canonical_url = _canonical_url(str(url))
            if canonical_url in seen_urls:
                continue
            seen_urls.add(canonical_url)
            summary = (
                item.get("description")
                or item.get("snippet")
                or item.get("summary")
                or _truncate(item.get("markdown"), 500)
                or metadata.get("description")
            )
            publisher = (
                item.get("source")
                or item.get("publisher")
                or metadata.get("siteName")
                or urlparse(canonical_url).netloc
            )
            published_at = _parse_time(
                item.get("date")
                or item.get("publishedDate")
                or metadata.get("publishedTime")
                or metadata.get("date")
            )
            out.append(
                SearchResult(
                    title=str(title),
                    url=canonical_url,
                    publisher=str(publisher) if publisher else None,
                    published_at=published_at,
                    summary=str(summary) if summary else None,
                    source_type="news_search" if bucket == "news" else "web_search",
                    credibility=_credibility(canonical_url, str(publisher) if publisher else None),
                    source_ref={
                        "provider": self.name,
                        "firecrawl_job_id": payload.get("id"),
                        "firecrawl_credits_used": payload.get("creditsUsed"),
                        "firecrawl_source": bucket,
                        "metadata": metadata,
                    },
                )
            )
            if len(out) >= max_results:
                break
        return out


async def _mark_started(pool: asyncpg.Pool, symbols_attempted: int) -> None:
    await pool.execute(
        """INSERT INTO source_health
             (source, last_started_at, last_status, symbols_attempted, updated_at)
           VALUES ($1, now(), 'running', $2, now())
           ON CONFLICT (source) DO UPDATE SET
               last_started_at = EXCLUDED.last_started_at,
               last_status = 'running',
               symbols_attempted = EXCLUDED.symbols_attempted,
               updated_at = now()""",
        SOURCE,
        symbols_attempted,
    )


async def _record_success(
    pool: asyncpg.Pool,
    *,
    rows_seen: int,
    rows_inserted: int,
    symbols_attempted: int,
    symbols_failed: int,
) -> None:
    status = "ok" if rows_inserted > 0 else "no_new_rows"
    await pool.execute(
        """INSERT INTO source_health
             (source, last_success_at, last_status, last_failure_kind,
              last_error, retry_after_at, rows_seen, rows_inserted,
              symbols_attempted, symbols_failed, updated_at)
           VALUES ($1, now(), $2, NULL, NULL, NULL, $3, $4, $5, $6, now())
           ON CONFLICT (source) DO UPDATE SET
               last_success_at = EXCLUDED.last_success_at,
               last_status = EXCLUDED.last_status,
               last_failure_kind = NULL,
               last_error = NULL,
               retry_after_at = NULL,
               rows_seen = EXCLUDED.rows_seen,
               rows_inserted = EXCLUDED.rows_inserted,
               symbols_attempted = EXCLUDED.symbols_attempted,
               symbols_failed = EXCLUDED.symbols_failed,
               updated_at = now()""",
        SOURCE,
        status,
        rows_seen,
        rows_inserted,
        symbols_attempted,
        symbols_failed,
    )


async def _record_failure(pool: asyncpg.Pool, error: str) -> None:
    await pool.execute(
        """INSERT INTO source_health
             (source, last_failure_at, last_status, last_failure_kind, last_error, updated_at)
           VALUES ($1, now(), 'failed', 'error', $2, now())
           ON CONFLICT (source) DO UPDATE SET
               last_failure_at = EXCLUDED.last_failure_at,
               last_status = EXCLUDED.last_status,
               last_failure_kind = EXCLUDED.last_failure_kind,
               last_error = EXCLUDED.last_error,
               updated_at = now()""",
        SOURCE,
        _truncate(error),
    )


async def _record_run(
    pool: asyncpg.Pool,
    *,
    symbol: str,
    provider: str,
    query: str,
    status: str,
    result_count: int,
    accepted_count: int = 0,
    rejected_count: int = 0,
    rejection_reasons: dict[str, int] | None = None,
    last_error: str | None = None,
) -> None:
    await pool.execute(
        """INSERT INTO research_retrieval_run
             (symbol, provider, query, status, result_count, last_error, source_ref)
           VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)""",
        symbol,
        provider,
        query,
        status,
        result_count,
        _truncate(last_error),
        json.dumps({
            "source": SOURCE,
            "accepted_results": accepted_count,
            "rejected_results": rejected_count,
            "rejection_reasons": rejection_reasons or {},
        }),
    )


async def _insert_result(
    pool: asyncpg.Pool,
    *,
    symbol: str,
    query: str,
    provider: str,
    result: SearchResult,
    tags: list[str],
    relevance: ResearchRelevance | None = None,
) -> bool:
    content_hash = hashlib.sha256(f"{symbol}|{result.url}".encode()).hexdigest()
    source_ref = dict(result.source_ref or {})
    if relevance is not None:
        source_ref["relevance"] = relevance.source_ref()
    row = await pool.fetchrow(
        """INSERT INTO research_evidence
             (symbol, query, url, title, publisher, published_at, provider,
              source_type, credibility, summary, tags, source_ref, content_hash)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::text[], $12::jsonb, $13)
           ON CONFLICT (symbol, url) DO UPDATE SET
              query = EXCLUDED.query,
              title = EXCLUDED.title,
              publisher = EXCLUDED.publisher,
              published_at = COALESCE(EXCLUDED.published_at, research_evidence.published_at),
              retrieved_at = now(),
              provider = EXCLUDED.provider,
              source_type = EXCLUDED.source_type,
              credibility = EXCLUDED.credibility,
              summary = COALESCE(EXCLUDED.summary, research_evidence.summary),
              tags = (
                  SELECT ARRAY(
                      SELECT DISTINCT unnest(research_evidence.tags || EXCLUDED.tags)
                  )
              ),
              source_ref = EXCLUDED.source_ref
        RETURNING id, (xmax = 0) AS inserted""",
        symbol,
        query,
        result.url,
        result.title,
        result.publisher,
        result.published_at,
        provider,
        result.source_type,
        result.credibility,
        result.summary,
        tags,
        json.dumps(source_ref),
        content_hash,
    )
    if not row:
        return False
    await _upsert_product_research_evidence_item(
        pool,
        research_id=int(row["id"]),
        symbol=symbol,
        query=query,
        provider=provider,
        result=result,
        tags=tags,
        relevance=relevance,
    )
    return bool(row["inserted"])


async def _upsert_product_research_evidence_item(
    pool: asyncpg.Pool,
    *,
    research_id: int,
    symbol: str,
    query: str,
    provider: str,
    result: SearchResult,
    tags: list[str],
    relevance: ResearchRelevance | None = None,
) -> None:
    observed_at = result.published_at or dt.datetime.now(dt.UTC)
    source_ref = {
        "table": "research_evidence",
        "id": research_id,
        "provider": provider,
        "query": query,
        "publisher": result.publisher,
        "credibility": result.credibility,
        "source_type": result.source_type,
        "tags": tags,
        **(result.source_ref or {}),
    }
    if relevance is not None:
        source_ref["relevance"] = relevance.source_ref()
    await pool.execute(
        """INSERT INTO evidence_item
             (symbol, kind, observed_at, source, source_id, source_ref,
              summary, strength, polarity, url)
           VALUES ($1, 'product_research', $2, $3, $4, $5::jsonb,
                   $6, $7, NULL, $8)
           ON CONFLICT (source, source_id) DO UPDATE SET
              observed_at = EXCLUDED.observed_at,
              source_ref = evidence_item.source_ref || EXCLUDED.source_ref,
              summary = EXCLUDED.summary,
              strength = EXCLUDED.strength,
              url = EXCLUDED.url,
              updated_at = now()""",
        symbol,
        observed_at,
        SOURCE,
        f"research_evidence:{research_id}",
        json.dumps(source_ref, default=str),
        _truncate(result.title, 500),
        _evidence_strength(result.credibility),
        result.url,
    )


async def _recent_run_exists(
    pool: asyncpg.Pool,
    symbol: str,
    *,
    max_age_hours: int,
    providers: list[str] | None = None,
) -> bool:
    if providers:
        rows = await pool.fetch(
            """SELECT DISTINCT provider
                 FROM research_retrieval_run
                WHERE symbol = $1
                  AND finished_at > now() - ($2::text || ' hours')::interval
                  AND provider = ANY($3::text[])""",
            symbol,
            str(max_age_hours),
            providers,
        )
        recent = {str(row["provider"]) for row in rows}
        return set(providers).issubset(recent)
    return bool(
        await pool.fetchval(
            """SELECT EXISTS (
                   SELECT 1
                     FROM research_retrieval_run
                    WHERE symbol = $1
                      AND finished_at > now() - ($2::text || ' hours')::interval
               )""",
            symbol,
            str(max_age_hours),
        )
    )


async def refresh_research_evidence(
    pool: asyncpg.Pool,
    symbol: str,
    *,
    context: dict | None = None,
    force: bool = False,
    disabled_providers: set[str] | None = None,
    extra_queries: list[str] | None = None,
) -> int:
    provider_setting = os.getenv("RESEARCH_PROVIDER", "gdelt,bing_news").lower()
    if provider_setting in {"", "off", "none"}:
        return 0
    max_age_hours = _env_int("RESEARCH_MAX_AGE_HOURS", 24)

    max_queries = max(1, _env_int("RESEARCH_MAX_QUERIES", 6))
    max_results = max(1, _env_int("RESEARCH_MAX_RESULTS_PER_QUERY", 5))
    min_interval_ms = max(0, _env_int("RESEARCH_MIN_REQUEST_INTERVAL_MS", 1500))
    profile = await _company_profile(pool, symbol)
    queries = build_queries(
        symbol,
        profile,
        context,
        max_queries=max_queries,
        extra_queries=extra_queries,
    )
    providers = []
    requested = {p.strip() for p in provider_setting.split(",")}
    if "gdelt" in requested or "gdelt_doc" in requested:
        providers.append(GdeltProvider())
    if "bing" in requested or "bing_news" in requested or "bing_news_rss" in requested:
        providers.append(BingNewsProvider())
    if "firecrawl" in requested:
        providers.append(FirecrawlProvider())
    if not providers:
        providers = [BingNewsProvider()]

    disabled_providers = set(disabled_providers or set())
    active_provider_names = [p.name for p in providers if p.name not in disabled_providers]
    if (
        not force
        and active_provider_names
        and await _recent_run_exists(
            pool,
            symbol,
            max_age_hours=max_age_hours,
            providers=active_provider_names,
        )
    ):
        for provider in providers:
            await provider.close()
        return 0

    rows_seen = 0
    rows_inserted = 0
    provider_failures = 0
    query_failures = 0
    await _mark_started(pool, 1)
    try:
        last_request_at: float | None = None
        for query in queries:
            query_had_success = False
            for provider in providers:
                if provider.name in disabled_providers:
                    continue
                if last_request_at is not None and min_interval_ms > 0:
                    elapsed = asyncio.get_running_loop().time() - last_request_at
                    wait = (min_interval_ms / 1000.0) - elapsed
                    if wait > 0:
                        await asyncio.sleep(wait)
                last_request_at = asyncio.get_running_loop().time()
                try:
                    results = await provider.search(query, max_results=max_results)
                    query_had_success = True
                    rows_seen += len(results)
                    accepted_results = 0
                    rejected_results = 0
                    rejection_reasons: dict[str, int] = {}
                    for result in results:
                        tags = [symbol, *(STATIC_PRODUCT_TERMS.get(symbol.upper(), [])[:5])]
                        relevance = research_relevance(symbol, profile, query, result, tags)
                        if not relevance.accepted:
                            rejected_results += 1
                            reason = relevance.rejected_reason or "rejected"
                            rejection_reasons[reason] = rejection_reasons.get(reason, 0) + 1
                            continue
                        accepted_results += 1
                        inserted = await _insert_result(
                            pool,
                            symbol=symbol,
                            query=query,
                            provider=provider.name,
                            result=result,
                            tags=tags,
                            relevance=relevance,
                        )
                        rows_inserted += int(inserted)
                    await _record_run(
                        pool,
                        symbol=symbol,
                        provider=provider.name,
                        query=query,
                        status="ok" if results else "no_results",
                        result_count=len(results),
                        accepted_count=accepted_results,
                        rejected_count=rejected_results,
                        rejection_reasons=rejection_reasons,
                    )
                    if accepted_results:
                        break
                except Exception as exc:  # noqa: BLE001
                    provider_failures += 1
                    error = str(exc)
                    await _record_run(
                        pool,
                        symbol=symbol,
                        provider=provider.name,
                        query=query,
                        status="failed",
                        result_count=0,
                        last_error=error,
                    )
                    if "429" in error or "too many requests" in error.lower():
                        disabled_providers.add(provider.name)
                    log.warning(
                        "research query failed symbol=%s provider=%s query=%r: %s",
                        symbol,
                        provider.name,
                            query,
                            exc,
                        )
            if not query_had_success:
                query_failures += 1
        await _record_success(
            pool,
            rows_seen=rows_seen,
            rows_inserted=rows_inserted,
            symbols_attempted=1,
            symbols_failed=1 if query_failures == len(queries) else 0,
        )
        if provider_failures:
            log.info(
                "research completed with provider failures symbol=%s failures=%d",
                symbol,
                provider_failures,
            )
        return rows_inserted
    except Exception as exc:  # noqa: BLE001
        await _record_failure(pool, str(exc))
        raise
    finally:
        for provider in providers:
            await provider.close()


async def load_research_evidence(
    pool: asyncpg.Pool,
    symbol: str,
    *,
    limit: int = 20,
) -> list[dict]:
    profile = await _company_profile(pool, symbol)
    rows = await pool.fetch(
        """WITH ranked AS (
              SELECT DISTINCT ON (lower(title), COALESCE(published_at, retrieved_at))
                     id, query, url, title, publisher, published_at, retrieved_at,
                     provider, source_type, credibility, summary, tags
                FROM research_evidence
               WHERE symbol = $1
            ORDER BY lower(title),
                     COALESCE(published_at, retrieved_at),
                     (url LIKE 'http://www.bing.com/%') ASC,
                     retrieved_at DESC
          )
          SELECT *
            FROM ranked
        ORDER BY credibility = 'primary' DESC,
                 published_at DESC NULLS LAST,
                 retrieved_at DESC
           LIMIT $2""",
        symbol,
        limit * 4,
    )
    out: list[dict] = []
    for r in rows:
        result = SearchResult(
            title=r["title"],
            url=r["url"],
            publisher=r["publisher"],
            published_at=r["published_at"],
            summary=r["summary"],
            source_type=r["source_type"],
            credibility=r["credibility"],
            source_ref={},
        )
        relevance = research_relevance(
            symbol,
            profile,
            r["query"],
            result,
            list(r["tags"] or []),
        )
        if not relevance.accepted:
            continue
        out.append({
            "id": r["id"],
            "query": r["query"],
            "url": r["url"],
            "title": r["title"],
            "publisher": r["publisher"],
            "published_at": r["published_at"].isoformat() if r["published_at"] else None,
            "retrieved_at": r["retrieved_at"].isoformat(),
            "provider": r["provider"],
            "source_type": r["source_type"],
            "credibility": r["credibility"],
            "summary": r["summary"],
            "tags": list(r["tags"] or []),
            "relevance": relevance.source_ref(),
        })
        if len(out) >= limit:
            break
    return out


async def _run_cli(symbol: str, *, force: bool) -> None:
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        await pool.execute(
            "INSERT INTO ticker (symbol) VALUES ($1) ON CONFLICT DO NOTHING",
            symbol,
        )
        inserted = await refresh_research_evidence(pool, symbol, force=force)
        rows = await load_research_evidence(pool, symbol)
        print(json.dumps({"inserted": inserted, "sources": rows}, indent=2, default=str))
    finally:
        await pool.close()


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="research")
    parser.add_argument("symbol", help="ticker symbol, e.g. AMD")
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    asyncio.run(_run_cli(args.symbol.upper(), force=args.force))


if __name__ == "__main__":
    _cli()
