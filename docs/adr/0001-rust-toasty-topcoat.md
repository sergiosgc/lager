# Rust with Toasty and Topcoat, pinned exactly

Lager replaces a Python CLI with a Rust web app built on Toasty (ORM) and Topcoat (full-stack web framework), both from tokio-rs. Both libraries are pre-1.0, and Topcoat calls itself "early-stage and experimental", so we accept frequent breaking changes in exchange for a single-language, server-rendered stack where Toasty and Topcoat are designed to work together. Both are pinned to exact versions (`=0.11.x`) and upgraded only deliberately.

## Consequences

- Topcoat 0.11 needs Rust ≥ 1.99.
- Recursive Inheritance queries go through Toasty's raw SQL, which returns untyped rows instead of models.
