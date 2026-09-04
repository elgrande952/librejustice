# ── health route contract tests ──────────────────────────────────────────────
# Source Rust: packages/lj-api/src/routes.rs (async fn health).
# DTO: HealthResponse { status: Literal["ok"], version: str } camelCase.
# Mounted under `/admin`: without the internal origin marker the route is a 404.

# Happy path: 200 + {status:"ok", version:<string>}
GET {{base}}/admin/health
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.status" exists
jsonpath "$.status" isString
jsonpath "$.status" == "ok"
jsonpath "$.version" exists
jsonpath "$.version" isString

# Idempotent: a second call returns the same structural contract.
GET {{base}}/admin/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString

# Trailing slash: the route is registered without one, so status only, no body.
GET {{base}}/admin/health/
HTTP *

# Wrong method on a GET-only route: both stacks should yield 405 Method Not Allowed.
POST {{base}}/admin/health
HTTP 405

# HEAD is auto-derived from GET on both frameworks (200, empty body).
HEAD {{base}}/admin/health
HTTP 200
