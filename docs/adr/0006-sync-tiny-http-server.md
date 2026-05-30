# A synchronous tiny_http server, not async/Tokio

The Server (ADR-0005) needs an HTTP listener, and the obvious Rust choice is
axum/hyper on Tokio — which is exactly why declining it needs recording. We use the
synchronous `tiny_http` instead: the poller thread writes a shared
`Arc<RwLock<Tracker>>` and a server thread reads it to answer requests, matching the
existing `std::thread` + blocking-`ureq` model (implementation plan, ADR-0001) and
keeping the size-optimized binary free of an async runtime. We serve only a handful
of read-only GET endpoints, so Tokio's ergonomics buy nothing here.

## Considered Options

- **axum + Tokio** — mainstream and SSE/WebSocket-ready, but pulls in a large async
  dependency tree and reverses the deliberate no-async architecture for no current
  need.
- **Hand-rolled on `std::net::TcpListener`** — zero dependencies, but reimplements
  HTTP/1.1 request parsing and is easy to get subtly wrong.
- **`tiny_http` (chosen)** — one small dependency, blocking, fits the thread model.

## Consequences

Revisit if we ever need server-push streaming (SSE/WebSocket) — that is the point at
which an async stack would begin to pay for itself.
