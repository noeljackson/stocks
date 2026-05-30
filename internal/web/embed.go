// Package web embeds the built Svelte SPA (SPEC §11: go:embed single binary).
// The dist/ dir holds a committed placeholder so this compiles before the first
// `make web-build`; the real build overwrites it.
package web

import "embed"

//go:embed all:dist
var Dist embed.FS
