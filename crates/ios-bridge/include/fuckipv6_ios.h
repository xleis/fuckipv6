/*
 * fuckipv6 iOS bridge - stable C ABI.
 *
 * Threading: every entry point except fv6_runtime_phase may block. Callers must
 * never invoke fv6_runtime_start/stop/snapshot/execute/network_path_changed or
 * the endpoint functions from a Tokio worker thread; the bridge rejects that
 * case with FV6_ERR_STATE instead of deadlocking.
 *
 * Memory: Fv6Runtime and Fv6Endpoint are opaque handles owned by the caller and
 * released exactly once. No function retains a caller buffer.
 *
 * Strings: UTF-8, NUL-terminated for inputs, length-delimited for outputs.
 *
 * Errors: functions return int32_t status codes. Functions that decode a
 * payload additionally write the payload length to out_len even when they fail
 * with FV6_ERR_BUFFER_TOO_SMALL, so callers can retry with a larger buffer.
 */

#ifndef FUCKIPV6_IOS_H
#define FUCKIPV6_IOS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Fv6Runtime Fv6Runtime;
typedef struct Fv6Endpoint Fv6Endpoint;

/* Status codes. Values are stable and must not be renumbered. */
enum Fv6Status {
    FV6_OK = 0,
    FV6_ERR_INVALID_ARGUMENT = 1,
    FV6_ERR_INVALID_HANDLE = 2,
    FV6_ERR_STATE = 3,
    FV6_ERR_IO = 4,
    FV6_ERR_TIMEOUT = 5,
    FV6_ERR_CLOSED = 6,
    FV6_ERR_BUFFER_TOO_SMALL = 7,
    FV6_ERR_QUEUE_FULL = 8,
    FV6_ERR_INVALID_PACKET = 9,
    FV6_ERR_PANIC = 10,
    FV6_ERR_BUSY = 11,
    FV6_ERR_NODE_PROBLEM = 12
};

/* Lifecycle phases reported by fv6_runtime_phase. */
enum Fv6Phase {
    FV6_PHASE_CREATED = 0,
    FV6_PHASE_STARTING = 1,
    FV6_PHASE_RUNNING = 2,
    FV6_PHASE_STOPPING = 3,
    FV6_PHASE_STOPPED = 4,
    FV6_PHASE_FAILED = 5
};

/*
 * Applies the derived tunnel configuration. settings_json is a UTF-8 JSON
 * object (see docs/IOS_P3_ARCHITECTURE.md). endpoint is a valid Fv6Endpoint
 * owned by the host from the moment the callback returns 0; on any other return
 * value ownership stays with Rust.
 *
 * The callback runs on a blocking thread. It may block for a bounded time.
 */
typedef int32_t (*Fv6OpenNetworkCallback)(void *context,
                                          const uint8_t *settings_json,
                                          size_t settings_len,
                                          Fv6Endpoint *endpoint);

/* Tears down the tunnel session. Must be idempotent. */
typedef int32_t (*Fv6CloseNetworkCallback)(void *context);

typedef struct Fv6Callbacks {
    Fv6OpenNetworkCallback open_network;
    Fv6CloseNetworkCallback close_network;
} Fv6Callbacks;

/*
 * config_json: {"storage_root":"/abs/path","dns_servers":["1.1.1.1"],
 *               "worker_threads":2}
 * context: opaque host pointer, must stay valid until fv6_runtime_destroy.
 * error_buffer/out_error_len receive a UTF-8 diagnostic on failure.
 */
int32_t fv6_runtime_create(const char *config_json,
                           Fv6Callbacks callbacks,
                           void *context,
                           Fv6Runtime **out_runtime,
                           uint8_t *error_buffer,
                           size_t error_buffer_len,
                           size_t *out_error_len);

void fv6_runtime_destroy(Fv6Runtime *runtime);

/* Bounded start. timeout_ms == 0 selects the 30 s default. Idempotent. */
int32_t fv6_runtime_start(Fv6Runtime *runtime, uint64_t timeout_ms);

/* Bounded stop. Cancels an in-flight start. Idempotent. */
int32_t fv6_runtime_stop(Fv6Runtime *runtime, uint64_t timeout_ms);

/* Current Fv6Phase, or -1 for a null handle. */
int32_t fv6_runtime_phase(const Fv6Runtime *runtime);

/* Last diagnostic as UTF-8; reports FV6_ERR_BUFFER_TOO_SMALL when short. */
int32_t fv6_runtime_last_error(const Fv6Runtime *runtime,
                               uint8_t *buffer,
                               size_t buffer_len,
                               size_t *out_len);

/* Serialises the authoritative NodeSnapshot as JSON. */
int32_t fv6_runtime_snapshot(Fv6Runtime *runtime,
                             uint8_t *buffer,
                             size_t buffer_len,
                             size_t *out_len,
                             uint64_t timeout_ms);

/* Accepts a NodeCommandRequest JSON and returns a NodeCommandOutcome JSON. */
int32_t fv6_runtime_execute(Fv6Runtime *runtime,
                            const char *request_json,
                            uint8_t *buffer,
                            size_t buffer_len,
                            size_t *out_len,
                            uint64_t timeout_ms);

/* Notifies the node that the host network path changed. */
int32_t fv6_runtime_network_path_changed(Fv6Runtime *runtime, uint64_t timeout_ms);

/* Endpoint handed to the host through Fv6OpenNetworkCallback. */
int32_t fv6_endpoint_mtu(const Fv6Endpoint *endpoint, uint16_t *out_mtu);

/* Queues one whole IP packet. Non-blocking: FV6_ERR_QUEUE_FULL means back off. */
int32_t fv6_endpoint_send(Fv6Endpoint *endpoint, const uint8_t *packet, size_t packet_len);

/*
 * Dequeues one whole IP packet. buffer_len must be at least the MTU.
 * FV6_ERR_TIMEOUT means the deadline elapsed without traffic.
 */
int32_t fv6_endpoint_recv(Fv6Endpoint *endpoint,
                          uint8_t *buffer,
                          size_t buffer_len,
                          size_t *out_len,
                          uint64_t timeout_ms);

/* Releases the endpoint. Safe on NULL, must be called exactly once. */
void fv6_endpoint_release(Fv6Endpoint *endpoint);

/* Creates the network configuration and identity under storage_root. */
int32_t fv6_prepare_network(const char *storage_root,
                            const char *network_name,
                            const char *coordinator_url,
                            int32_t set_default,
                            uint8_t *error_buffer,
                            size_t error_buffer_len,
                            size_t *out_error_len);

/* 1 for a snapshot outcome, 2 for any other outcome. */
int32_t fv6_outcome_kind(const uint8_t *buffer, size_t buffer_len, int32_t *out_kind);

#ifdef __cplusplus
}
#endif

#endif /* FUCKIPV6_IOS_H */
