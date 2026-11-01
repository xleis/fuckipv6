#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT_DIR="${PUBLIC_API_SECURITY_REPORT_DIR:-${ROOT}/target/public-api-security}"

cd "${ROOT}"
mkdir -p "${REPORT_DIR}"

# These exact tests cover the externally reachable HTTP/WebSocket boundary:
# authentication bypass, query-token leakage, CSRF/origin checks, proxy-header
# spoofing, body-size exhaustion, command concurrency exhaustion, schema
# downgrade, and unknown-command fail-closed behavior.
TESTS=(
  "unauthenticated and invalid bearer tokens|webui::tests::protected_http_api_rejects_missing_and_invalid_tokens"
  "query-string tokens rejected|webui::tests::protected_http_api_does_not_accept_tokens_in_query_strings"
  "node routes bearer-only|webui::tests::node_routes_accept_bearer_only_and_reject_query_tokens"
  "websocket anonymous rejected|webui::tests::websocket_rejects_anonymous_upgrade_when_auth_is_required"
  "websocket bearer subprotocol only|webui::tests::websocket_bearer_uses_subprotocol_header_not_query_string"
  "login rate limit|webui::tests::login_endpoint_rate_limits_failed_attempts_per_source_ip"
  "command rate limit|webui::tests::authenticated_command_rate_limit_is_per_source_and_independent_from_login"
  "anonymous mutation origin|webui::tests::anonymous_local_mutations_still_require_same_origin"
  "owner cookie CSRF|webui::tests::local_mutation_csrf_accepts_same_origin_owner_cookie_and_rejects_cross_site"
  "public proxy and host validation|webui::public_claim::tests::rejects_host_origin_and_forwarded_for_injection"
  "plaintext and ambiguous proxy headers|webui::public_claim::tests::rejects_plaintext_ambiguous_and_untrusted_proxy_requests"
  "direct TLS proxy headers|webui::public_claim::tests::direct_tls_uses_peer_address_and_rejects_proxy_headers"
  "command body limit|node::http_adapter::tests::command_endpoints_reject_oversized_json_before_deserialization"
  "command concurrency limit|node::http_adapter::tests::command_endpoints_fail_fast_when_concurrency_limit_is_exhausted"
  "command permit release|node::http_adapter::tests::command_concurrency_permit_is_released_after_success_and_rejection"
  "legacy schema downgrade|node::http_adapter::tests::command_endpoint_rejects_schema_mismatch_before_bus_submission"
  "v1 schema downgrade|node::http_adapter::tests::v1_routes_reject_schema_mismatch_with_meta_envelope"
  "unknown command fail closed|node::contract::tests::unknown_command_values_fail_closed_on_deserialization"
)

printf 'public API attack-surface tests: %d cases\n' "${#TESTS[@]}"
for entry in "${TESTS[@]}"; do
  label="${entry%%|*}"
  test_name="${entry#*|}"
  log="${REPORT_DIR}/${test_name//::/_}.log"

  printf '\n==> %s\n    %s\n' "${label}" "${test_name}"
  set +e
  CARGO_TERM_COLOR=never cargo test \
    -p fuckipv6-client \
    --no-default-features \
    --lib \
    "${test_name}" \
    -- \
    --exact \
    --nocapture 2>&1 | tee "${log}"
  status=${PIPESTATUS[0]}
  set -e

  if ((status != 0)); then
    printf 'attack-surface test failed: %s\nreport: %s\n' "${test_name}" "${log}" >&2
    exit "${status}"
  fi
  if ! grep -Eq '^test result: ok\. 1 passed; 0 failed' "${log}"; then
    printf 'attack-surface test did not execute exactly one passing test: %s\nreport: %s\n' \
      "${test_name}" "${log}" >&2
    exit 1
  fi
done

printf '\npublic API attack-surface tests passed; reports: %s\n' "${REPORT_DIR}"
