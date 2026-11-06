#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEBUI_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(WEBUI_ROOT, "..");
const DIST_ROOT = join(REPO_ROOT, "crates/client/src/webui/dist");
const FIXTURE_ROOT = join(REPO_ROOT, "contracts/typescript/fixtures");
const REPORT_ROOT = resolve(
  process.env.TOPOLOGY_MEMBER_E2E_OUTPUT_DIR ??
    join(REPO_ROOT, "target/topology-member-e2e"),
);
const DESKTOP_VIEWPORT = {
  width: 1440,
  height: 900,
  deviceScaleFactor: 1,
};
const MOBILE_VIEWPORT = {
  width: 480,
  height: 900,
  deviceScaleFactor: 1,
};
const FIXED_SERVER_TIME = "2026-01-01T00:00:05Z";
const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

function websocketTextFrame(payload) {
  const body = Buffer.from(payload);
  if (body.length < 126) {
    return Buffer.concat([Buffer.from([0x81, body.length]), body]);
  }
  if (body.length < 65_536) {
    const header = Buffer.alloc(4);
    header[0] = 0x81;
    header[1] = 126;
    header.writeUInt16BE(body.length, 2);
    return Buffer.concat([header, body]);
  }
  const header = Buffer.alloc(10);
  header[0] = 0x81;
  header[1] = 127;
  header.writeBigUInt64BE(BigInt(body.length), 2);
  return Buffer.concat([header, body]);
}

function commandAvailable(command) {
  return spawnSync(command, ["--version"], { stdio: "ignore" }).status === 0;
}

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    "chrome",
    "google-chrome-stable",
    "google-chrome",
    "chromium",
    "chromium-browser",
  ].filter(Boolean);
  const chrome = candidates.find(commandAvailable);
  if (!chrome) throw new Error("Chrome/Chromium not found; set CHROME_BIN");
  return chrome;
}

function run(command, args, options) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { ...options, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} exited with ${signal ?? code}`));
    });
  });
}

async function prepareSharedAssets(destination) {
  await run(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "fuckipv6-common",
      "--example",
      "export-browser-assets",
      "--",
      destination,
    ],
    { cwd: REPO_ROOT },
  );
}

function jsonResponse(response, value, status = 200) {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end(JSON.stringify(value));
}

function fileResponse(response, body, contentType) {
  response.writeHead(200, {
    "content-type": contentType,
    "cache-control": "no-store",
  });
  response.end(body);
}

function contentType(pathname) {
  switch (extname(pathname)) {
    case ".css":
      return "text/css; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    default:
      return "application/octet-stream";
  }
}

const SELF_ID = `node_${"A".repeat(43)}`;
const BOB_ID = `node_${"B".repeat(43)}`;
const CAROL_ID = `node_${"C".repeat(43)}`;
const APPROVE_AUDIT_ID = "audit_approve_bob";
const REMOVE_AUDIT_ID = "audit_remove_bob";
const APPROVE_OPERATION_ID = "operation_approve_bob";
const REMOVE_OPERATION_ID = "operation_remove_bob";

function presence(state, observedAt = 1_700_000_000_000) {
  return {
    state,
    source: state === "online" ? "live_session" : "not_observed",
    observed_at_unix_ms: state === "unknown" ? null : observedAt,
  };
}

function topologyFixture() {
  return {
    revision: 12,
    viewer: SELF_ID,
    generated_at_unix_ms: 1_700_000_001_000,
    truncated: false,
    boundaries: [
      {
        network_id: "home",
        name: "Home",
        member_count: 2,
        visible_member_count: 2,
        viewer_role: "owner",
        capabilities: ["relay"],
      },
      {
        network_id: "studio",
        name: "Studio",
        member_count: 2,
        visible_member_count: 2,
        viewer_role: "administrator",
        capabilities: ["exit"],
      },
    ],
    clusters: [],
    nodes: [
      {
        node_id: SELF_ID,
        display_name: "Local node",
        health: "healthy",
        is_self: true,
        capabilities: ["relay"],
        networks: [
          {
            network_id: "home",
            health: "healthy",
            presence: presence("online"),
            role: "owner",
            status: "active",
            capabilities: ["relay"],
          },
        ],
      },
      {
        node_id: BOB_ID,
        display_name: "Bob",
        health: "healthy",
        is_self: false,
        capabilities: ["exit", "file"],
        networks: [
          {
            network_id: "home",
            health: "healthy",
            presence: presence("online"),
            role: "member",
            status: "pending",
            capabilities: ["file"],
          },
          {
            network_id: "studio",
            health: "healthy",
            presence: presence("online"),
            role: "member",
            status: "active",
            capabilities: ["exit"],
          },
        ],
      },
      {
        node_id: CAROL_ID,
        display_name: "Carol",
        health: "healthy",
        is_self: false,
        capabilities: ["coordinator"],
        networks: [
          {
            network_id: "studio",
            health: "healthy",
            presence: presence("online"),
            role: "administrator",
            status: "active",
            capabilities: ["coordinator"],
          },
        ],
      },
    ],
    connections: [
      {
        connection_id: "connection-self-bob",
        from: SELF_ID,
        to: BOB_ID,
        networks: ["home", "studio"],
        observed_at_unix_ms: 1_700_000_000_400,
        state: "active",
      },
      {
        connection_id: "connection-bob-carol",
        from: BOB_ID,
        to: CAROL_ID,
        networks: ["studio"],
        observed_at_unix_ms: 1_700_000_000_300,
        state: "idle",
      },
    ],
  };
}

function createJourneyState(productSnapshot, topology) {
  return {
    snapshot: productSnapshot,
    topology,
    homeMemberRevision: 12,
    homeBobStatus: "pending",
    homeBobLifecycle: "pending_approval",
    homeBobPresence: presence("online"),
    groupPolicy: {
      network_id: "home",
      policy_id: "policy_group_media_share",
      revision: 1,
      bob_group: "file-readers",
      bob_capabilities: ["file"],
      viewer_role: "owner",
      can_manage_members: true,
    },
    commands: [],
    outcomes: [],
    reconnects: 0,
    lastRemovedNodeId: null,
  };
}

function membershipForNode(node, networkId) {
  return node.networks.find((network) => network.network_id === networkId);
}

function refreshBoundaryCounts(topology) {
  for (const boundary of topology.boundaries) {
    const members = topology.nodes.filter((node) =>
      node.networks.some((network) => network.network_id === boundary.network_id),
    );
    boundary.member_count = members.length;
    boundary.visible_member_count = members.length;
  }
}

function bumpAuthority(state, increment = 1, generatedIncrement = 1_000) {
  state.snapshot.revision += increment;
  state.topology.revision += increment;
  state.topology.generated_at_unix_ms += generatedIncrement;
}

function setHomeBobState(state, { status, lifecycle, health }) {
  const bob = state.topology.nodes.find((node) => node.node_id === BOB_ID);
  if (!bob) throw new Error("fixture Bob node is missing");
  const home = membershipForNode(bob, "home");
  if (!home) throw new Error("fixture Bob home membership is missing");
  home.status = status;
  home.health = health;
  bob.health = health;
  state.homeBobStatus = status;
  state.homeBobLifecycle = lifecycle;
  if (status === "active") home.presence = presence("online");
  if (status === "degraded") home.presence = presence("online");
  state.homeBobPresence = home.presence;
}

function updateHomeGroupPolicy(
  state,
  { bobGroup, bobCapabilities, viewerRole, canManageMembers },
) {
  const bob = state.topology.nodes.find((node) => node.node_id === BOB_ID);
  if (!bob) throw new Error("fixture Bob node is missing");
  const home = membershipForNode(bob, "home");
  if (!home) throw new Error("fixture Bob home membership is missing");
  state.groupPolicy = {
    ...state.groupPolicy,
    revision: state.groupPolicy.revision + 1,
    bob_group: bobGroup,
    bob_capabilities: [...bobCapabilities],
    viewer_role: viewerRole,
    can_manage_members: canManageMembers,
  };
  home.capabilities = [...bobCapabilities];
  bob.capabilities = [
    ...new Set(bob.networks.flatMap((network) => network.capabilities ?? [])),
  ].sort();
  bumpAuthority(state);
}

function removeBobFromHome(state) {
  const bob = state.topology.nodes.find((node) => node.node_id === BOB_ID);
  if (!bob) throw new Error("fixture Bob node is missing");
  bob.networks = bob.networks.filter((network) => network.network_id !== "home");
  bob.health = "healthy";
  state.topology.connections = state.topology.connections.filter(
    (connection) =>
      connection.connection_id !== "connection-self-bob" ||
      !connection.networks.includes("home"),
  ).map((connection) =>
    connection.connection_id === "connection-self-bob"
      ? { ...connection, networks: ["studio"] }
      : connection,
  );
  state.homeBobStatus = "removed";
  state.homeBobLifecycle = "removed";
  state.homeMemberRevision = 15;
  refreshBoundaryCounts(state.topology);
}

function memberFromNode(state, node, networkId) {
  const network = membershipForNode(node, networkId);
  if (!network) return null;
  const isHomeBob = node.node_id === BOB_ID && networkId === "home";
  const status = isHomeBob ? state.homeBobStatus : network.status;
  const lifecycle = isHomeBob ? state.homeBobLifecycle : network.status;
  const revision = isHomeBob ? state.homeMemberRevision : 7;
  const capabilities = isHomeBob
    ? state.groupPolicy.bob_capabilities
    : network.capabilities;
  const canManageHome = state.groupPolicy.can_manage_members;
  const canManage = networkId === "studio" ? true : canManageHome;
  const availableActions =
    isHomeBob && status === "pending"
      ? ["approve", "reject"]
      : [SELF_ID, CAROL_ID].includes(node.node_id)
        ? []
        : ["suspend", "remove"];
  return {
    node_id: node.node_id,
    display_name: node.display_name,
    identity_fingerprint: `fingerprint-${node.node_id.slice(-8)}`,
    role: network.role,
    groups: [network.role],
    capabilities,
    status,
    lifecycle_state: lifecycle,
    revision,
    access_epoch: 4,
    ...(canManage ? { available_actions: availableActions } : {}),
    presence: isHomeBob ? state.homeBobPresence : network.presence,
  };
}

function memberDirectory(state, networkId) {
  const members = state.topology.nodes
    .map((node) => memberFromNode(state, node, networkId))
    .filter(Boolean);
  return {
    network_id: networkId,
    generated_at_unix_ms: state.topology.generated_at_unix_ms,
    viewer_role:
      networkId === "studio"
        ? "administrator"
        : state.groupPolicy.viewer_role,
    can_manage_members:
      networkId === "studio" ? true : state.groupPolicy.can_manage_members,
    members,
  };
}

function removalPreview(state) {
  return {
    network_id: "home",
    target_node_id: BOB_ID,
    display_name: "Bob",
    role: "member",
    status: state.homeBobStatus,
    lifecycle_state: state.homeBobLifecycle,
    revision: state.homeMemberRevision,
    access_epoch: 4,
    presence: state.homeBobPresence,
    can_remove: state.homeBobStatus === "degraded",
    network_access_revoked: true,
    file_access_revoked: true,
    incomplete_transfers_resume_blocked: true,
    credentials_revoked: true,
    local_files_deleted: false,
    old_access_epoch_invalidated: true,
    safe_undo_requires_reapproval: true,
    safe_undo_restores_old_credentials: false,
    impact_keys: [
      "member_removal.impact.network_access",
      "member_removal.impact.file_access",
      "member_removal.impact.incomplete_transfers",
      "member_removal.impact.credentials",
      "member_removal.impact.local_files_kept",
      "member_removal.impact.safe_undo",
    ],
  };
}

function clonePayload(payload) {
  return JSON.parse(JSON.stringify(payload ?? {}));
}

async function startFixtureServer(manifest, fixture, sharedAssets, state) {
  const appHtml = await readFile(join(DIST_ROOT, "index.html"));
  const webuiCss = await readFile(join(sharedAssets, "webui.css"));
  const i18nJs = await readFile(join(sharedAssets, "i18n.js"));
  const distAssets = new Map(
    await Promise.all(
      (await readdir(join(DIST_ROOT, "assets"))).map(async (name) => [
        `/assets/${name}`,
        await readFile(join(DIST_ROOT, "assets", name)),
      ]),
    ),
  );
  const clients = new Set();
  const sockets = new Set();
  let snapshotRequests = 0;
  let manifestRequests = 0;
  let websocketConnections = 0;

  function broadcastSnapshot() {
    const payload = JSON.stringify({
      schema_version: 1,
      type: "snapshot",
      snapshot: state.snapshot,
    });
    const frame = websocketTextFrame(payload);
    for (const client of clients) client.write(frame);
    return clients.size;
  }

  function resultOutcome(envelope, outcome) {
    return {
      schema_version: 1,
      result: {
        request_id: envelope.request.request_id,
        outcome,
      },
    };
  }

  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (url.pathname === "/api/v1/commands" && request.method === "POST") {
      let body = "";
      request.setEncoding("utf8");
      request.on("data", (chunk) => {
        body += chunk;
      });
      request.on("end", () => {
        try {
          const envelope = JSON.parse(body);
          const commandRequest = envelope?.request?.command;
          const command =
            typeof commandRequest === "string"
              ? commandRequest
              : commandRequest?.command;
          const payload = clonePayload(commandRequest?.payload ?? {});
          state.commands.push({
            schema_version: envelope?.schema_version,
            request_id: envelope?.request?.request_id,
            idempotency_key: envelope?.idempotency_key,
            command: String(command),
            payload,
          });
          const networkId = String(payload.network_id ?? "home");
          if (command === "create_invitation") {
            jsonResponse(response, resultOutcome(envelope, {
              status: "invitation_created",
              data: {
                code: `fkinv_${"A".repeat(43)}`,
                status: {
                  invitation: {
                    invitation_id: "inv_home_member",
                    network_id: "home",
                    created_by: SELF_ID,
                    role: "member",
                    capabilities: ["file"],
                    max_uses: 2,
                    used_count: 0,
                    created_at_unix_ms: 1_700_000_000_000,
                    expires_at_unix_ms: 1_700_003_600_000,
                    revision: 3,
                  },
                  state: "active",
                  remaining_uses: 2,
                },
              },
            }));
            return;
          }
          if (command === "list_network_members") {
            jsonResponse(response, resultOutcome(envelope, {
              status: "network_members",
              data: memberDirectory(state, networkId),
            }));
            return;
          }
          if (command === "preview_member_removal") {
            if (
              payload.target_node_id !== BOB_ID ||
              state.homeBobStatus !== "degraded"
            ) {
              throw new Error("fixture removal preview requires degraded Bob on home");
            }
            jsonResponse(response, resultOutcome(envelope, {
              status: "member_removal_preview",
              data: removalPreview(state),
            }));
            return;
          }
          if (command === "change_member_lifecycle") {
            const expectedRevision = Number(payload.expected_revision);
            if (payload.target_node_id !== BOB_ID) {
              throw new Error("fixture change requires Bob");
            }
            if (payload.action === "approve") {
              if (
                state.homeBobStatus !== "pending" ||
                expectedRevision !== state.homeMemberRevision
              ) {
                throw new Error("fixture approve revision is stale");
              }
              state.homeMemberRevision = 13;
              setHomeBobState(state, {
                status: "active",
                lifecycle: "active",
                health: "healthy",
              });
              bumpAuthority(state);
              const data = {
                network_id: "home",
                target_node_id: BOB_ID,
                action: "approve",
                status: "active",
                revision: 13,
                access_epoch: 4,
                operation_id: APPROVE_OPERATION_ID,
                audit_id: APPROVE_AUDIT_ID,
                idempotent: false,
                access_revoked: false,
                requires_reauthentication: true,
                requires_reapproval: false,
              };
              state.outcomes.push({
                command: "change_member_lifecycle",
                status: "member_lifecycle_changed",
                data,
              });
              jsonResponse(response, resultOutcome(envelope, {
                status: "member_lifecycle_changed",
                data,
              }));
              setImmediate(broadcastSnapshot);
              return;
            }
            if (payload.action === "remove") {
              if (
                state.homeBobStatus !== "degraded" ||
                expectedRevision !== state.homeMemberRevision
              ) {
                throw new Error("fixture remove revision is stale");
              }
              removeBobFromHome(state);
              bumpAuthority(state);
              state.snapshot.recent_activity = [
                {
                  audit_id: REMOVE_AUDIT_ID,
                  actor: SELF_ID,
                  action: "member.remove",
                  resource: `${BOB_ID}@home`,
                  outcome: "succeeded",
                  occurred_at_unix_ms: 1_700_000_200_000,
                  summary: "Member removed",
                  network_id: "home",
                  details: { command: "change_member_lifecycle" },
                },
                ...(state.snapshot.recent_activity ?? []),
              ];
              const data = {
                network_id: "home",
                target_node_id: BOB_ID,
                action: "remove",
                status: "removed",
                revision: 15,
                access_epoch: 5,
                operation_id: REMOVE_OPERATION_ID,
                audit_id: REMOVE_AUDIT_ID,
                idempotent: false,
                access_revoked: true,
                requires_reauthentication: true,
                requires_reapproval: true,
              };
              state.outcomes.push({
                command: "change_member_lifecycle",
                status: "member_lifecycle_changed",
                data,
              });
              jsonResponse(response, resultOutcome(envelope, {
                status: "member_lifecycle_changed",
                data,
              }));
              setImmediate(broadcastSnapshot);
              return;
            }
          }
          jsonResponse(
            response,
            {
              schema_version: 1,
              error: {
                code: "unsupported_fixture_command",
                message: `fixture does not implement ${String(command)}`,
              },
            },
            400,
          );
        } catch (error) {
          jsonResponse(
            response,
            {
              schema_version: 1,
              error: {
                code: "invalid_fixture_command",
                message: error instanceof Error ? error.message : "invalid fixture command",
              },
            },
            400,
          );
        }
      });
      return;
    }

    switch (url.pathname) {
      case "/":
        fileResponse(response, appHtml, "text/html; charset=utf-8");
        return;
      case "/assets/webui.css":
        fileResponse(response, webuiCss, "text/css; charset=utf-8");
        return;
      case "/assets/i18n.js":
        fileResponse(response, i18nJs, "text/javascript; charset=utf-8");
        return;
      case "/api/v1/auth/session":
        jsonResponse(response, { authenticated: true });
        return;
      case "/api/v1/settings/language":
        jsonResponse(response, { preference: fixture.language_preference });
        return;
      case "/api/v1/manifest":
        manifestRequests += 1;
        jsonResponse(response, {
          schema_version: 1,
          request_id: manifestRequests,
          server_time: FIXED_SERVER_TIME,
          manifest,
        });
        return;
      case "/api/v1/snapshot":
        snapshotRequests += 1;
        jsonResponse(response, {
          schema_version: 1,
          request_id: snapshotRequests,
          server_time: FIXED_SERVER_TIME,
          snapshot: state.snapshot,
        });
        return;
      default: {
        const asset = distAssets.get(url.pathname);
        if (asset) {
          fileResponse(response, asset, contentType(url.pathname));
          return;
        }
        response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
        response.end("not found");
      }
    }
  });

  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (request, socket) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    const key = request.headers["sec-websocket-key"];
    if (url.pathname !== "/api/node/events" || typeof key !== "string") {
      socket.destroy();
      return;
    }
    const accept = createHash("sha1").update(key + WEBSOCKET_GUID).digest("base64");
    socket.write(
      "HTTP/1.1 101 Switching Protocols\r\n" +
        "Upgrade: websocket\r\n" +
        "Connection: Upgrade\r\n" +
        `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    websocketConnections += 1;
    const connectionNumber = websocketConnections;
    clients.add(socket);
    socket.on("error", () => {});
    socket.once("close", () => clients.delete(socket));
    if (connectionNumber > 1) {
      state.reconnects += 1;
      setImmediate(() => {
        if (socket.destroyed) return;
        socket.write(
          websocketTextFrame(
            JSON.stringify({
              schema_version: 1,
              type: "snapshot",
              snapshot: state.snapshot,
            }),
          ),
        );
      });
    }
  });

  await new Promise((resolvePromise, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolvePromise);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("fixture server did not bind TCP");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    state,
    broadcastSnapshot,
    commandCount(command) {
      return state.commands.filter((entry) => entry.command === command).length;
    },
    snapshotRequestCount() {
      return snapshotRequests;
    },
    manifestRequestCount() {
      return manifestRequests;
    },
    websocketConnectionCount() {
      return websocketConnections;
    },
    connectedClientCount() {
      return clients.size;
    },
    disconnectClients() {
      const count = clients.size;
      for (const client of clients) client.destroy();
      return count;
    },
    close: async () => {
      for (const client of clients) client.destroy();
      for (const socket of sockets) socket.destroy();
      await new Promise((resolvePromise) => server.close(resolvePromise));
    },
  };
}

async function stopChild(child) {
  if (!child || child.exitCode !== null) return;
  const exited = new Promise((resolvePromise) => child.once("exit", resolvePromise));
  child.kill("SIGTERM");
  await Promise.race([
    exited,
    new Promise((resolvePromise) => setTimeout(resolvePromise, 2_000)),
  ]);
  if (child.exitCode === null) {
    child.kill("SIGKILL");
    await exited;
  }
}

async function waitForFile(path, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      return await readFile(path, "utf8");
    } catch {
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
    }
  }
  throw new Error(`timed out waiting for ${path}`);
}

function sleep(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

class CdpSession {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(`${pending.method}: ${message.error.message}`));
      else pending.resolve(message.result);
    });
    socket.addEventListener("close", () => {
      for (const pending of this.pending.values()) {
        pending.reject(new Error(`${pending.method}: CDP socket closed`));
      }
      this.pending.clear();
    });
  }

  static async connect(url) {
    const socket = new WebSocket(url);
    await new Promise((resolvePromise, reject) => {
      socket.addEventListener("open", resolvePromise, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    return new CdpSession(socket);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    this.socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { method, resolve: resolvePromise, reject });
    });
  }

  close() {
    this.socket.close();
  }
}

async function evaluate(session, expression) {
  const result = await session.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  }
  return result.result.value;
}

async function waitFor(session, expression, description, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(session, expression)) return;
    } catch (error) {
      lastError = error;
    }
    await sleep(50);
  }
  throw new Error(
    `timed out waiting for ${description}${lastError ? `: ${lastError.message}` : ""}`,
  );
}

async function waitForReady(session, timeoutMs = 20_000) {
  await waitFor(
    session,
    `Boolean(
      document.querySelector(".app-shell") &&
      document.querySelector(".nav-footer")?.textContent?.includes("Live")
    )`,
    "WebUI product shell",
    timeoutMs,
  );
  await evaluate(session, "document.fonts.ready.then(() => true)");
  await sleep(250);
}

async function activeElement(session) {
  return evaluate(
    session,
    `(() => {
      const element = document.activeElement;
      if (!element) return null;
      return {
        tag: element.tagName.toLowerCase(),
        role: element.getAttribute("role"),
        text: element.textContent?.trim().slice(0, 180) ?? "",
        aria_label: element.getAttribute("aria-label"),
        test_id: element.getAttribute("data-testid"),
        node_id: element.getAttribute("data-node-id"),
        class_name: element.getAttribute("class"),
      };
    })()`,
  );
}

async function waitForActive(session, predicate, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const info = await activeElement(session);
    if (await evaluate(
      session,
      `(() => { const el = document.activeElement; return Boolean(${predicate}); })()`,
    )) {
      return info;
    }
    await sleep(50);
  }
  throw new Error(`timed out waiting for focus on ${description}: ${JSON.stringify(await activeElement(session))}`);
}

async function openNodeMenuByKeyboard(session, nodeId) {
  await evaluate(
    session,
    `document.querySelector('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]')?.focus()`,
  );
  await waitForActive(
    session,
    `el?.matches('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]')`,
    `${nodeId} keyboard focus`,
  );
  await pressKey(session, "F10", { shift: true });
  await waitFor(
    session,
    `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
    `${nodeId} keyboard menu`,
  );
}

async function pressKey(session, key, options = {}) {
  const definitions = {
    Tab: { code: "Tab", virtualKeyCode: 9 },
    Enter: { code: "Enter", virtualKeyCode: 13, text: "\r" },
    Escape: { code: "Escape", virtualKeyCode: 27 },
    " ": { code: "Space", virtualKeyCode: 32, text: " " },
    ArrowLeft: { code: "ArrowLeft", virtualKeyCode: 37 },
    ArrowUp: { code: "ArrowUp", virtualKeyCode: 38 },
    ArrowRight: { code: "ArrowRight", virtualKeyCode: 39 },
    ArrowDown: { code: "ArrowDown", virtualKeyCode: 40 },
    Home: { code: "Home", virtualKeyCode: 36 },
    End: { code: "End", virtualKeyCode: 35 },
    F10: { code: "F10", virtualKeyCode: 121 },
  };
  const definition = definitions[key];
  if (!definition) throw new Error(`unsupported key ${key}`);
  const modifiers = options.shift ? 8 : 0;
  const common = {
    key,
    code: definition.code,
    modifiers,
    windowsVirtualKeyCode: definition.virtualKeyCode,
    nativeVirtualKeyCode: definition.virtualKeyCode,
    ...(definition.text ? { text: definition.text, unmodifiedText: definition.text } : {}),
  };
  await session.send("Input.dispatchKeyEvent", { type: "keyDown", ...common });
  await session.send("Input.dispatchKeyEvent", { type: "keyUp", ...common });
}

async function tabTo(session, predicate, description, maxTabs = 100) {
  for (let index = 0; index < maxTabs; index += 1) {
    await pressKey(session, "Tab");
    const matched = await evaluate(
      session,
      `(() => { const el = document.activeElement; return Boolean(${predicate}); })()`,
    );
    if (matched) return activeElement(session);
  }
  throw new Error(`could not reach ${description} with Tab; focus is ${JSON.stringify(await activeElement(session))}`);
}

async function dispatchTouchLongPress(session, selector) {
  const point = await evaluate(
    session,
    `(async () => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) return null;
      element.scrollIntoView({ block: "center", inline: "center" });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const rect = element.getBoundingClientRect();
      const x = rect.left + rect.width / 2;
      const y = rect.top + rect.height / 2;
      const hit = document.elementFromPoint(x, y);
      if (!hit || !element.contains(hit)) {
        throw new Error("touch target is not visible after scrolling into view");
      }
      return { x, y };
    })()`,
  );
  if (!point) throw new Error(`touch target not found: ${selector}`);
  await session.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [{ x: point.x, y: point.y, radiusX: 2, radiusY: 2, force: 1 }],
  });
  await sleep(560);
  await session.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
}

function gitValue(args) {
  const result = spawnSync("git", args, { cwd: REPO_ROOT, encoding: "utf8" });
  if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed`);
  return result.stdout.trim();
}

async function waitForCondition(predicate, description, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await sleep(50);
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function waitForCommandAfter(
  fixtureServer,
  startIndex,
  command,
  predicate,
  description,
  timeoutMs = 10_000,
) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const match = fixtureServer.state.commands
      .slice(startIndex)
      .find((entry) => entry.command === command && predicate(entry));
    if (match) return match;
    await sleep(50);
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function dispatchMouse(session, type, x, y, options = {}) {
  await session.send("Input.dispatchMouseEvent", { type, x, y, ...options });
}

async function clickPoint(session, point, modifiers = 0) {
  await dispatchMouse(session, "mouseMoved", point.x, point.y, {
    button: "none",
    buttons: 0,
  });
  await dispatchMouse(session, "mousePressed", point.x, point.y, {
    button: "left",
    buttons: 1,
    clickCount: 1,
    modifiers,
  });
  await dispatchMouse(session, "mouseReleased", point.x, point.y, {
    button: "left",
    buttons: 0,
    clickCount: 1,
    modifiers,
  });
}

async function clickElement(session, expression, description) {
  await waitFor(session, `Boolean(${expression})`, description, 8_000);
  const point = await evaluate(
    session,
    `(async () => {
      const element = ${expression};
      if (!element) throw new Error(${JSON.stringify(`${description} is missing`)});
      element.scrollIntoView({ block: "center", inline: "center" });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const bounds = element.getBoundingClientRect();
      const point = { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 };
      const hit = document.elementFromPoint(point.x, point.y);
      if (hit !== element && !element.contains(hit)) {
        throw new Error(${JSON.stringify(`${description} is occluded`)});
      }
      return point;
    })()`,
  );
  await clickPoint(session, point);
}

async function rightClick(session, point) {
  await dispatchMouse(session, "mouseMoved", point.x, point.y, {
    button: "none",
    buttons: 0,
  });
  await dispatchMouse(session, "mousePressed", point.x, point.y, {
    button: "right",
    buttons: 2,
    clickCount: 1,
  });
  await dispatchMouse(session, "mouseReleased", point.x, point.y, {
    button: "right",
    buttons: 0,
    clickCount: 1,
  });
}

async function nodeCenter(session, nodeId) {
  return evaluate(
    session,
    `(() => {
      const node = document.querySelector('g.topology-node[data-node-id=${JSON.stringify(nodeId)}]');
      if (!node) throw new Error("topology node ${nodeId} is missing");
      node.scrollIntoView({ block: "center", inline: "center" });
      const bounds = node.getBoundingClientRect();
      const point = { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 };
      const hit = document.elementFromPoint(point.x, point.y)?.closest("g.topology-node");
      if (hit?.getAttribute("data-node-id") !== ${JSON.stringify(nodeId)}) {
        throw new Error("topology node ${nodeId} is occluded");
      }
      return point;
    })()`,
  );
}

function controlInCard(cardExpression, labelText) {
  return `(() => {
    const card = ${cardExpression};
    if (!card) return null;
    const field = [...card.querySelectorAll("label")].find(
      (label) => label.querySelector("span")?.textContent?.trim() === ${JSON.stringify(labelText)},
    );
    return field?.querySelector("input, select, textarea") ?? null;
  })()`;
}

async function setControlValue(
  session,
  expression,
  value,
  description,
  assertCurrentValue = true,
) {
  await waitFor(session, `Boolean(${expression})`, description, 8_000);
  await evaluate(
    session,
    `(() => {
      const element = ${expression};
      if (!element) throw new Error(${JSON.stringify(`${description} is missing`)});
      element.focus();
      const prototype = element instanceof HTMLSelectElement
        ? HTMLSelectElement.prototype
        : element instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
      if (!setter) throw new Error("value setter is unavailable");
      setter.call(element, ${JSON.stringify(value)});
      element.dispatchEvent(new Event("input", { bubbles: true }));
      element.dispatchEvent(new Event("change", { bubbles: true }));
      return element.value;
    })()`,
  );
  if (assertCurrentValue) {
    assert.equal(await evaluate(session, `(${expression})?.value`), value, description);
  }
}

function actionCardExpression(labelText) {
  return `[...document.querySelectorAll("article.action-card")].find(
    (card) => card.querySelector(".action-card-head")?.textContent?.trim() === ${JSON.stringify(labelText)}
  )`;
}

function pageTabExpression(labelText) {
  return `[...document.querySelectorAll('.page-tabs [role="tab"]')].find(
    (tab) => tab.textContent?.trim() === ${JSON.stringify(labelText)}
  )`;
}

function moduleButtonExpression(labelText) {
  return `[...document.querySelectorAll(".module-button")].find(
    (button) => button.textContent?.trim() === ${JSON.stringify(labelText)}
  )`;
}

async function navigateToNetworkTabWithKeyboard(session, tabLabel, readyExpression, description) {
  await tabTo(
    session,
    `el?.classList.contains("module-button") && el.textContent?.trim() === "Networks"`,
    "Networks module",
  );
  await pressKey(session, "Enter");
  await waitFor(
    session,
    `[...document.querySelectorAll('[role="tab"]')].some((tab) => tab.textContent?.trim() === ${JSON.stringify(tabLabel)})`,
    `${tabLabel} page tab`,
  );
  await tabTo(
    session,
    `el?.getAttribute("role") === "tab" && el.textContent?.trim() === ${JSON.stringify(tabLabel)}`,
    `${tabLabel} tab`,
  );
  await pressKey(session, "Enter");
  await waitFor(session, readyExpression, description);
}

async function navigateToNetworkTabByClick(session, tabLabel, readyExpression, description) {
  await clickElement(session, moduleButtonExpression("Networks"), "Networks module");
  await waitFor(
    session,
    `[...document.querySelectorAll('[role="tab"]')].some((tab) => tab.textContent?.trim() === ${JSON.stringify(tabLabel)})`,
    `${tabLabel} page tab`,
  );
  await clickElement(session, pageTabExpression(tabLabel), `${tabLabel} tab`);
  await waitFor(session, readyExpression, description);
}

async function navigateToOverviewByClick(session) {
  await clickElement(session, moduleButtonExpression("Overview"), "Overview module");
  await waitFor(session, `Boolean(document.querySelector(".overview-stack"))`, "overview page");
}

async function selectTopologyFilterNetwork(session, networkId, visible, total) {
  const selector = `(() => {
    const label = [...document.querySelectorAll(".topology-filters label")].find(
      (candidate) => candidate.querySelector("span")?.textContent?.trim() === "Network",
    );
    return label?.querySelector("select") ?? null;
  })()`;
  await setControlValue(session, selector, networkId, `topology network filter ${networkId}`);
  await waitFor(
    session,
    `document.querySelector(".topology-filter-count")?.textContent?.trim() === ${JSON.stringify(
      `Showing ${visible} of ${total} nodes`,
    )}`,
    `topology network filter result for ${networkId}`,
    8_000,
  );
}

async function selectTopologySearch(session, value) {
  const selector = `document.querySelector('.topology-filters input[type="search"]')`;
  await setControlValue(session, selector, value, `topology search ${value}`);
}

async function selectGovernanceNetwork(session, fixtureServer, networkId) {
  const startIndex = fixtureServer.state.commands.length;
  const selector = `document.querySelector('[data-testid="topology-governance"] select')`;
  try {
    await setControlValue(
      session,
      selector,
      networkId,
      `governance network ${networkId}`,
      false,
    );
  } catch (error) {
    const diagnostic = await evaluate(
      session,
      `(() => ({
        governance: Boolean(document.querySelector('[data-testid="topology-governance"]')),
        governanceText: document.querySelector('[data-testid="topology-governance"]')?.innerText?.slice(0, 800) ?? null,
        selectionText: [...document.querySelectorAll('.topology-selection, [class*="selection"]')].map((node) => node.textContent?.trim()).filter(Boolean).slice(0, 8),
        topologyText: document.querySelector('.topology-page')?.innerText?.slice(0, 2_000) ?? null,
      }))()`,
    );
    throw new Error(`${error.message}; diagnostic=${JSON.stringify(diagnostic)}`);
  }
  return waitForCommandAfter(
    fixtureServer,
    startIndex,
    "list_network_members",
    (entry) => entry.payload.network_id === networkId,
    `member directory refresh for ${networkId}`,
    12_000,
  );
}

async function main() {
  if (!process.argv.includes("--skip-build")) {
    await run("npm", ["run", "build"], { cwd: WEBUI_ROOT });
  }
  await mkdir(REPORT_ROOT, { recursive: true });
  const workspace = await mkdtemp(join(tmpdir(), "fuckipv6-topology-member-e2e-"));
  const chromeProfile = join(workspace, "chrome-profile");
  const sharedAssets = join(workspace, "shared-assets");
  const checks = [];
  let fixtureServer;
  let chrome;
  let chromeStderr = "";
  let cdp;
  let state;
  try {
    const manifest = JSON.parse(await readFile(join(FIXTURE_ROOT, "node-manifest.json"), "utf8"));
    const fixture = JSON.parse(
      await readFile(join(FIXTURE_ROOT, "product-visual-snapshot.json"), "utf8"),
    );
    const topology = topologyFixture();
    fixture.snapshot.topology = topology;
    state = createJourneyState(fixture.snapshot, topology);
    await prepareSharedAssets(sharedAssets);
    fixtureServer = await startFixtureServer(manifest, fixture, sharedAssets, state);

    chrome = spawn(
      findChrome(),
      [
        "--headless=new",
        "--remote-debugging-port=0",
        `--user-data-dir=${chromeProfile}`,
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-default-apps",
        "--disable-extensions",
        "--disable-sync",
        "--metrics-recording-only",
        "--mute-audio",
        "--no-sandbox",
        "--hide-scrollbars",
        "about:blank",
      ],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    chrome.stderr.on("data", (chunk) => {
      chromeStderr = `${chromeStderr}${chunk}`.slice(-8_000);
    });
    const devtoolsPort = await waitForFile(join(chromeProfile, "DevToolsActivePort"));
    const port = Number(devtoolsPort.split("\n")[0]);
    if (!Number.isInteger(port) || port <= 0) throw new Error("invalid Chrome DevTools port");

    const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) =>
      response.json(),
    );
    const target = targets.find((candidate) => candidate.type === "page");
    if (!target) throw new Error("Chrome has no page target");
    cdp = await CdpSession.connect(target.webSocketDebuggerUrl);

    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...DESKTOP_VIEWPORT,
      screenWidth: DESKTOP_VIEWPORT.width,
      screenHeight: DESKTOP_VIEWPORT.height,
      mobile: false,
    });
    await cdp.send("Emulation.setLocaleOverride", { locale: "en-US" });
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "UTC" });
    await cdp.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-reduced-motion", value: "reduce" }],
    });
    await cdp.send("Page.navigate", { url: `${fixtureServer.origin}/` });
    await waitForReady(cdp);

    await navigateToNetworkTabWithKeyboard(
      cdp,
      "Networks",
      `Boolean(${actionCardExpression("Create invitation")})`,
      "Networks action cards",
    );
    checks.push("keyboard-only navigation reaches the Networks governance workspace");

    const createInvitationCard = actionCardExpression("Create invitation");
    await clickElement(
      cdp,
      `${createInvitationCard}?.querySelector(".action-card-head")`,
      "Create invitation action",
    );
    await setControlValue(
      cdp,
      controlInCard(createInvitationCard, "Network ID"),
      "home",
      "invitation network",
    );
    await setControlValue(
      cdp,
      controlInCard(createInvitationCard, "Invited role"),
      "member",
      "invitation role",
    );
    await setControlValue(
      cdp,
      controlInCard(createInvitationCard, "Capability declarations"),
      "file",
      "invitation capabilities",
    );
    await setControlValue(
      cdp,
      controlInCard(createInvitationCard, "Maximum uses"),
      "2",
      "invitation maximum uses",
    );
    await setControlValue(
      cdp,
      controlInCard(createInvitationCard, "Expires in (ms)"),
      "3600000",
      "invitation expiry",
    );
    await clickElement(
      cdp,
      `(${createInvitationCard})?.querySelector('form.action-form button[type="submit"]')`,
      "Run create invitation",
    );
    await waitFor(cdp, `Boolean(document.querySelector('[role="dialog"]'))`, "invitation confirmation");
    await clickElement(
      cdp,
      `[...document.querySelectorAll('[role="dialog"] button')].find(
        (button) => button.textContent?.trim() === "Run"
      )`,
      "confirm create invitation",
    );
    await waitFor(
      cdp,
      `Boolean((${createInvitationCard})?.querySelector('[data-testid="invitation-created"]'))`,
      "created invitation evidence",
    );
    const invitationCommand = fixtureServer.state.commands.find(
      (entry) => entry.command === "create_invitation",
    );
    assert.ok(invitationCommand, "invitation command was not submitted");
    assert.deepEqual(invitationCommand.payload, {
      network_id: "home",
      role: "member",
      capabilities: ["file"],
      max_uses: 2,
      expires_in_ms: 3_600_000,
    });
    let invitationCode = await evaluate(
      cdp,
      `(${createInvitationCard})?.querySelector('[data-testid="invitation-code"]')?.textContent?.trim()`,
    );
    assert.equal(
      /^fkinv_[A-Za-z0-9_-]{43}$/.test(invitationCode),
      true,
      "invalid invitation code format",
    );
    invitationCode = null;
    checks.push("invitation creation is submitted from the UI and the one-time code matches the private format");

    const initialDirectoryStart = fixtureServer.state.commands.length;
    await navigateToNetworkTabWithKeyboard(
      cdp,
      "Topology",
      `Boolean(document.querySelector(".topology-view"))`,
      "topology view",
    );
    await waitForCommandAfter(
      fixtureServer,
      initialDirectoryStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "initial home member directory",
    );
    await waitFor(
      cdp,
      `document.querySelectorAll("g.topology-node[role=option]").length === 3 &&
       Boolean(document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]'))`,
      "initial topology graph",
    );
    await evaluate(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.focus()`,
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-node-detail"]')?.textContent?.includes("Pending") &&
       document.querySelector('[data-testid="topology-node-detail"]')?.textContent?.includes("Online")`,
      "Bob pending home membership",
    );
    await pressKey(cdp, " ");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected") === "true" &&
       Boolean(document.querySelector('[data-testid="topology-governance"] select'))`,
      "Bob batch selection and governance controls",
    );
    checks.push("Bob is pending and online in the authoritative home membership");

    await selectGovernanceNetwork(cdp, fixtureServer, "studio");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("aria-label")?.includes("Studio")`,
      "Bob studio identity remains visible",
    );
    await evaluate(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.focus()`,
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-node-detail"]')?.textContent?.includes("Active")`,
      "Bob active studio membership",
    );
    checks.push("shared Bob node preserves independent pending home and active studio memberships");
    await pressKey(cdp, " ");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected") === "true" &&
       Boolean(document.querySelector('[data-testid="topology-governance"] select'))`,
      "Bob studio batch selection",
    );
    await selectGovernanceNetwork(cdp, fixtureServer, "home");

    await selectTopologyFilterNetwork(cdp, "studio", 2, 3);
    assert.deepEqual(
      await evaluate(
        cdp,
        `[...document.querySelectorAll("g.topology-node[data-node-id]")].map(
          (node) => node.getAttribute("data-node-id")
        ).sort()`,
      ),
      [BOB_ID, CAROL_ID].sort(),
    );
    await selectTopologyFilterNetwork(cdp, "all", 3, 3);
    checks.push("cross-network filtering shows only Studio objects and restores the complete graph");

    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...MOBILE_VIEWPORT,
      screenWidth: MOBILE_VIEWPORT.width,
      screenHeight: MOBILE_VIEWPORT.height,
      mobile: true,
    });
    await cdp.send("Emulation.setTouchEmulationEnabled", {
      enabled: true,
      maxTouchPoints: 1,
    });
    await cdp.send("Page.reload", { ignoreCache: true });
    await waitForReady(cdp);
    await navigateToNetworkTabWithKeyboard(
      cdp,
      "Topology",
      `document.querySelector(".topology-view")?.dataset.view === "list"`,
      "narrow topology list",
    );
    await waitFor(
      cdp,
      `document.querySelectorAll("tbody tr[data-node-id]").length === 3`,
      "three compact topology rows",
    );
    await dispatchTouchLongPress(cdp, `tr[data-node-id="${BOB_ID}"]`);
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
      "touch long-press topology menu",
      5_000,
    );
    await pressKey(cdp, "Escape");
    checks.push("narrow layout uses the compact list and a real touch long-press opens the shared menu");

    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...DESKTOP_VIEWPORT,
      screenWidth: DESKTOP_VIEWPORT.width,
      screenHeight: DESKTOP_VIEWPORT.height,
      mobile: false,
    });
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: false });
    await cdp.send("Page.reload", { ignoreCache: true });
    await waitForReady(cdp);
    await navigateToNetworkTabWithKeyboard(
      cdp,
      "Topology",
      `document.querySelector(".topology-view")?.dataset.view === "graph"`,
      "desktop topology graph",
    );
    await waitFor(
      cdp,
      `document.querySelectorAll("g.topology-node[role=option]").length === 3`,
      "desktop graph after mobile check",
    );

    await sleep(200);
    await rightClick(cdp, await nodeCenter(cdp, BOB_ID));
    await waitFor(
      cdp,
      `(() => {
        const menu = document.querySelector('[data-testid="topology-node-menu"]');
        return menu?.textContent?.includes("Bob") && menu?.textContent?.includes("Approve");
      })()`,
      "right-click topology menu with loaded member actions",
    );
    await pressKey(cdp, "Escape");
    await waitFor(
      cdp,
      `!document.querySelector('[data-testid="topology-node-menu"]')`,
      "right-click menu close",
    );
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${BOB_ID}"]')`,
      "Bob node after right-click menu",
    );
    checks.push("real pointer right-click opens the same Bob action menu");

    const approveStart = fixtureServer.state.commands.length;
    await pressKey(cdp, "F10", { shift: true });
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-node-menu"]'))`,
      "keyboard topology menu",
    );
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem" && el.textContent?.trim() === "Open node details"`,
      "first keyboard topology menu item",
    );
    await pressKey(cdp, "End");
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem" && el.textContent?.trim() === "Reject"`,
      "last lifecycle menu item",
    );
    await pressKey(cdp, "ArrowUp");
    await waitForActive(
      cdp,
      `el?.getAttribute("role") === "menuitem" && el.textContent?.trim() === "Approve"`,
      "Approve lifecycle menu item",
    );
    await pressKey(cdp, "Enter");
    await waitFor(
      cdp,
      `!document.querySelector('[data-testid="topology-node-menu"]') &&
       document.querySelector('[data-testid="topology-governance"]')?.textContent?.includes("Approve")`,
      "Approve preparation in topology governance",
    );
    await clickElement(
      cdp,
      `[...document.querySelectorAll("#topology-governance button")].find(
        (button) => button.textContent?.trim() === "Preview and confirm" && !button.disabled
      )`,
      "preview Approve batch",
    );
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-batch-dialog"]'))`,
      "Approve confirmation dialog",
    );
    await clickElement(
      cdp,
      `[...document.querySelectorAll('[data-testid="topology-batch-dialog"] button')].find(
        (button) => button.textContent?.trim().startsWith("Execute ")
      )`,
      "execute Approve batch",
    );
    try {
      await waitFor(
        cdp,
        `Boolean(document.querySelector('[data-testid="topology-batch-results"] [data-testid="topology-batch-result-${BOB_ID}"].success')) &&
         document.querySelector('[data-testid="topology-batch-result-${BOB_ID}"]')?.textContent?.includes(${JSON.stringify(APPROVE_AUDIT_ID)})`,
        "audited Approve batch result",
        12_000,
      );
    } catch (error) {
      const diagnostic = await evaluate(
        cdp,
        `(() => ({
          results: document.querySelector('[data-testid="topology-batch-results"]')?.innerText ?? null,
          dialog: document.querySelector('[data-testid="topology-batch-dialog"]')?.innerText ?? null,
          feedback: document.querySelector('[data-testid="topology-governance"]')?.innerText ?? null,
        }))()`,
      );
      throw new Error(
        `${error.message}; fixture=${JSON.stringify({
          homeBobStatus: state.homeBobStatus,
          homeMemberRevision: state.homeMemberRevision,
          outcomes: state.outcomes,
        })}; commands=${JSON.stringify(fixtureServer.state.commands.slice(approveStart))}; diagnostic=${JSON.stringify(diagnostic)}`,
      );
    }
    const approveCommand = await waitForCommandAfter(
      fixtureServer,
      approveStart,
      "change_member_lifecycle",
      (entry) => entry.payload.action === "approve",
      "Approve lifecycle command",
    );
    assert.deepEqual(approveCommand.payload, {
      network_id: "home",
      target_node_id: BOB_ID,
      action: "approve",
      role: "member",
      expected_revision: 12,
    });
    const approveOutcome = state.outcomes.find(
      (entry) => entry.data?.audit_id === APPROVE_AUDIT_ID,
    );
    assert.equal(approveOutcome?.data?.audit_id, APPROVE_AUDIT_ID);
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("aria-label")?.includes("Healthy") &&
       document.querySelector('[data-testid="topology-node-detail"]')?.textContent?.includes("Active")`,
      "Bob active after approval",
    );
    checks.push("Shift+F10, End, ArrowUp and Enter prepare and execute one audited Approve action");
    checks.push("Approval per-item result and authoritative topology both expose the same audit ID");

    const postApprovalDirectoryStart = fixtureServer.state.commands.length;
    await clickElement(
      cdp,
      `[...document.querySelectorAll('[data-testid="topology-batch-results"] button')].find(
        (button) => button.textContent?.trim() === "Finish and clear selection"
      )`,
      "finish approved batch results",
    );
    await waitForCommandAfter(
      fixtureServer,
      postApprovalDirectoryStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "home directory refresh after finishing approval",
    );
    await evaluate(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.focus()`,
    );
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${BOB_ID}"]')`,
      "Bob node before group policy check",
    );
    await pressKey(cdp, " ");
    try {
      await waitFor(
        cdp,
        `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected") === "true" &&
         Boolean(document.querySelector('[data-testid="topology-governance"]'))`,
        "Bob selection before group policy check",
      );
    } catch (error) {
      const diagnostic = await evaluate(
        cdp,
        `(() => ({
          selected: document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected"),
          governance: document.querySelector('#topology-governance')?.innerText ?? null,
          batchResults: document.querySelector('[data-testid="topology-batch-results"]')?.innerText ?? null,
          active: document.activeElement?.getAttribute("data-node-id") ?? document.activeElement?.textContent?.trim().slice(0, 100) ?? null,
        }))()`,
      );
      throw new Error(`${error.message}; diagnostic=${JSON.stringify(diagnostic)}`);
    }

    assert.match(
      await evaluate(
        cdp,
        `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("aria-label") ?? ""`,
      ),
      /Exit.*File|File.*Exit/,
      "Bob topology does not expose the initial group-granted File capability",
    );
    const groupPolicyDirectoryRequestsBefore = fixtureServer.commandCount(
      "list_network_members",
    );
    updateHomeGroupPolicy(state, {
      bobGroup: "relay-operators",
      bobCapabilities: ["relay"],
      viewerRole: "member",
      canManageMembers: false,
    });
    const restrictedDirectoryStart = fixtureServer.state.commands.length;
    fixtureServer.broadcastSnapshot();
    await waitFor(
      cdp,
      `(() => {
        const label = document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("aria-label") ?? "";
        const detail = document.querySelector('[data-testid="topology-node-detail"]')?.textContent ?? "";
        return label.includes("Relay") && !label.includes("File") &&
          detail.includes("Relay") && !detail.includes("File");
      })()`,
      "topology capabilities after restricted group policy",
      12_000,
    );
    await waitForCommandAfter(
      fixtureServer,
      restrictedDirectoryStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "home directory refresh after restricted group policy",
    );
    try {
      await waitFor(
        cdp,
        `(() => {
          const governance = document.querySelector('[data-testid="topology-governance"]');
          const text = governance?.textContent ?? "";
          const selectedBob = document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected") === "true";
          return selectedBob && governance?.getAttribute("data-testid") === "topology-governance" &&
            text.includes("No server-authorized actions") &&
            !text.includes("member directory is stale") &&
            !text.includes("Loading the member directory");
        })()`,
        "restricted directory capabilities settled",
        12_000,
      );
    } catch (error) {
      const diagnostic = await evaluate(
        cdp,
        `(() => ({
          selectedBob: document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected"),
          governanceText: document.querySelector('#topology-governance')?.innerText ?? null,
          governanceHtml: document.querySelector('#topology-governance')?.innerHTML?.slice(0, 2_000) ?? null,
          actionOptions: [...document.querySelectorAll('#topology-governance select option')].map((option) => option.textContent?.trim()),
        }))()`,
      );
      throw new Error(`${error.message}; diagnostic=${JSON.stringify(diagnostic)}`);
    }
    await openNodeMenuByKeyboard(cdp, BOB_ID);
    await waitFor(
      cdp,
      `document.querySelector('[data-testid="topology-node-menu"] [role="menuitem"][aria-disabled="true"]')?.textContent?.trim() === "The server exposes no actions for this object"`,
      "lifecycle capabilities after restricted group policy",
    );
    await pressKey(cdp, "Escape");

    const restoredDirectoryStart = fixtureServer.state.commands.length;
    updateHomeGroupPolicy(state, {
      bobGroup: "file-readers",
      bobCapabilities: ["file"],
      viewerRole: "member",
      canManageMembers: true,
    });
    fixtureServer.broadcastSnapshot();
    await waitFor(
      cdp,
      `(() => {
        const label = document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("aria-label") ?? "";
        return label.includes("File") && label.includes("Exit");
      })()`,
      "topology capabilities after restored group policy",
      12_000,
    );
    await waitForCommandAfter(
      fixtureServer,
      restoredDirectoryStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "home directory refresh after restored group policy",
    );
    await openNodeMenuByKeyboard(cdp, BOB_ID);
    await waitFor(
      cdp,
      `["Suspend", "Remove"].every((action) =>
        [...document.querySelectorAll('[data-testid="topology-node-menu"] [role="menuitem"]')].some(
          (item) => item.textContent?.trim() === action
        )
      )`,
      "lifecycle capabilities after restored group policy",
    );
    await pressKey(cdp, "Escape");
    const groupPolicyDirectoryRequests = fixtureServer.commandCount(
      "list_network_members",
    ) - groupPolicyDirectoryRequestsBefore;
    assert.equal(
      groupPolicyDirectoryRequests,
      2,
      `group policy changes caused ${groupPolicyDirectoryRequests} directory refreshes instead of 2`,
    );
    checks.push("group policy changes refresh topology capabilities and lifecycle controls from authority");

    state.homeMemberRevision = 14;
    setHomeBobState(state, {
      status: "degraded",
      lifecycle: "degraded",
      health: "degraded",
    });
    const degradedDirectoryStart = fixtureServer.state.commands.length;
    bumpAuthority(state);
    fixtureServer.broadcastSnapshot();
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("aria-label")?.includes("Degraded") &&
       document.querySelector('[data-testid="topology-node-detail"]')?.textContent?.includes("Degraded")`,
      "authoritative degraded Bob state",
    );
    await waitForCommandAfter(
      fixtureServer,
      degradedDirectoryStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "degraded home directory refresh",
    );
    await rightClick(cdp, await nodeCenter(cdp, BOB_ID));
    await waitFor(
      cdp,
      `[...document.querySelectorAll('[data-testid="topology-node-menu"] [role="menuitem"]')].some(
        (item) => item.textContent?.trim() === "Remove"
      )`,
      "Remove action after degradation",
    );
    await pressKey(cdp, "Escape");
    checks.push("authoritative degradation immediately refreshes Bob and exposes the Remove action");

    const membersStart = fixtureServer.state.commands.length;
    await clickElement(cdp, pageTabExpression("Networks"), "Networks page tab");
    await waitFor(
      cdp,
      `Boolean(${actionCardExpression("View members")})`,
      "View members action",
    );
    const viewMembersCard = actionCardExpression("View members");
    await clickElement(
      cdp,
      `${viewMembersCard}?.querySelector(".action-card-head")`,
      "View members action",
    );
    await setControlValue(
      cdp,
      controlInCard(viewMembersCard, "Network ID"),
      "home",
      "member directory network",
    );
    await clickElement(
      cdp,
      `(${viewMembersCard})?.querySelector('form.action-form button[type="submit"]')`,
      "load home members",
    );
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="member-list"]'))`,
      "home member directory",
      12_000,
    );
    const listMembersCommand = await waitForCommandAfter(
      fixtureServer,
      membersStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "home member directory command",
    );
    assert.deepEqual(listMembersCommand.payload, { network_id: "home" });
    const bobOption = `[...document.querySelectorAll('[data-testid="member-list"] [role="option"]')].find(
      (option) => option.textContent?.includes("Bob")
    )`;
    await clickElement(cdp, bobOption, "Bob member option");
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="member-governance"]')) &&
       Boolean(document.querySelector('[data-testid="member-governance-action-remove"]'))`,
      "member governance controls",
    );
    const previewStart = fixtureServer.state.commands.length;
    await clickElement(
      cdp,
      `document.querySelector('[data-testid="member-governance-action-remove"]')`,
      "Remove member action",
    );
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="member-governance-dialog"]')) &&
       document.querySelectorAll('[data-testid="member-removal-impact"] li').length === 6`,
      "removal impact dialog",
      12_000,
    );
    const previewCommand = await waitForCommandAfter(
      fixtureServer,
      previewStart,
      "preview_member_removal",
      (entry) => entry.payload.target_node_id === BOB_ID,
      "member removal preview command",
    );
    assert.deepEqual(previewCommand.payload, {
      network_id: "home",
      target_node_id: BOB_ID,
    });
    assert.equal(
      await evaluate(
        cdp,
        `document.querySelectorAll('[data-testid="member-removal-impact"] li').length`,
      ),
      6,
    );
    const removeStart = fixtureServer.state.commands.length;
    await clickElement(
      cdp,
      `document.querySelector('[data-testid="member-governance-confirm"]')`,
      "confirm member removal",
    );
    const removeCommand = await waitForCommandAfter(
      fixtureServer,
      removeStart,
      "change_member_lifecycle",
      (entry) => entry.payload.action === "remove",
      "remove lifecycle command",
      12_000,
    );
    assert.deepEqual(removeCommand.payload, {
      network_id: "home",
      target_node_id: BOB_ID,
      action: "remove",
      role: "member",
      expected_revision: 14,
    });
    const removalRefreshCommand = await waitForCommandAfter(
      fixtureServer,
      removeStart + 1,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "authoritative member directory refresh after removal",
      12_000,
    );
    assert.deepEqual(removalRefreshCommand.payload, { network_id: "home" });
    try {
      await waitFor(
        cdp,
        `!document.querySelector('[data-testid="member-governance-dialog"]') &&
         document.querySelectorAll('[data-testid="member-list"] [role="option"]').length === 1 &&
         ![...document.querySelectorAll('[data-testid="member-list"] [role="option"]')].some(
           (option) => option.textContent?.includes("Bob")
         ) &&
         [...document.querySelectorAll('.action-card .form-success')].some(
           (node) => node.textContent?.trim() ===
             "Member directory refreshed: 1 members · 1 online or confirmed connected"
         )`,
        "member removal and refreshed directory status",
        12_000,
      );
    } catch (error) {
      const diagnostic = await evaluate(
        cdp,
        `(() => ({
          feedback: document.querySelector('[data-testid="member-governance-feedback"]')?.innerText ?? null,
          memberList: document.querySelector('[data-testid="member-list"]')?.innerText ?? null,
          successMessages: [...document.querySelectorAll('.action-card .form-success')].map(
            (node) => node.textContent?.trim()
          ),
        }))()`,
      );
      throw new Error(
        `${error.message}; fixture=${JSON.stringify({
          homeBobStatus: state.homeBobStatus,
          homeMemberRevision: state.homeMemberRevision,
          outcomes: state.outcomes,
        })}; commands=${JSON.stringify(fixtureServer.state.commands.slice(removeStart))}; diagnostic=${JSON.stringify(diagnostic)}`,
      );
    }
    const removeOutcome = state.outcomes.find(
      (entry) => entry.data?.audit_id === REMOVE_AUDIT_ID,
    );
    assert.equal(removeOutcome?.data?.audit_id, REMOVE_AUDIT_ID);
    checks.push("member directory preview lists all six impacts and removal refreshes Bob out of home");
    checks.push("directory removal uses the preview revision and returns the authoritative removal audit ID");

    const topologyAfterRemovalStart = fixtureServer.state.commands.length;
    await clickElement(cdp, pageTabExpression("Topology"), "Topology page tab");
    await waitFor(
      cdp,
      `Boolean(document.querySelector(".topology-view"))`,
      "topology after removal",
    );
    await waitForCommandAfter(
      fixtureServer,
      topologyAfterRemovalStart,
      "list_network_members",
      (entry) => entry.payload.network_id === "home",
      "home directory after removal",
    );
    await selectTopologyFilterNetwork(cdp, "home", 1, 3);
    assert.equal(
      await evaluate(
        cdp,
        `document.querySelectorAll("g.topology-node[data-node-id]").length === 1 &&
         !document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')`,
      ),
      true,
    );
    await selectTopologyFilterNetwork(cdp, "studio", 2, 3);
    assert.deepEqual(
      await evaluate(
        cdp,
        `[...document.querySelectorAll("g.topology-node[data-node-id]")].map(
          (node) => node.getAttribute("data-node-id")
        ).sort()`,
      ),
      [BOB_ID, CAROL_ID].sort(),
    );
    await selectTopologyFilterNetwork(cdp, "all", 3, 3);
    await openNodeMenuByKeyboard(cdp, BOB_ID);
    await waitFor(
      cdp,
      `Boolean(document.querySelector('[data-testid="topology-node-menu"] [role="menuitem"][aria-disabled="true"]'))`,
      "revoked home menu actions",
    );
    assert.equal(
      await evaluate(
        cdp,
        `document.querySelector('[data-testid="topology-node-menu"] [role="menuitem"][aria-disabled="true"]')?.textContent?.trim()`,
      ),
      "The server exposes no actions for this object",
    );
    await pressKey(cdp, "Escape");
    await waitFor(
      cdp,
      `!document.querySelector('[data-testid="topology-node-menu"]')`,
      "revoked home menu close",
    );
    await evaluate(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.focus()`,
    );
    await waitForActive(
      cdp,
      `el?.matches('g.topology-node[data-node-id="${BOB_ID}"]')`,
      "Bob node before Studio governance",
    );
    await pressKey(cdp, " ");
    await waitFor(
      cdp,
      `document.querySelector('g.topology-node[data-node-id="${BOB_ID}"]')?.getAttribute("data-selected") === "true" &&
       Boolean(document.querySelector('[data-testid="topology-governance"] select'))`,
      "Bob Studio batch selection",
    );
    await selectGovernanceNetwork(cdp, fixtureServer, "studio");
    await openNodeMenuByKeyboard(cdp, BOB_ID);
    await waitFor(
      cdp,
      `[...document.querySelectorAll('[data-testid="topology-node-menu"] [role="menuitem"]')].some(
        (item) => item.textContent?.trim() === "Remove"
      )`,
      "Studio lifecycle actions remain available",
    );
    await pressKey(cdp, "Escape");
    checks.push("revoked home access removes Bob from home while the shared Studio object and actions remain");

    await navigateToOverviewByClick(cdp);
    await waitFor(
      cdp,
      `Boolean(document.querySelector('.overview-activity-row[data-audit-id="${REMOVE_AUDIT_ID}"]'))`,
      "removal activity row",
      12_000,
    );
    const activityText = await evaluate(
      cdp,
      `document.querySelector('.overview-activity-row[data-audit-id="${REMOVE_AUDIT_ID}"]')?.textContent ?? ""`,
    );
    assert.match(activityText, /change_member_lifecycle/);
    assert.equal(removeOutcome?.data?.audit_id, REMOVE_AUDIT_ID);
    checks.push("removal response and Overview activity are traceable by the same audit ID and command");

    await navigateToNetworkTabByClick(
      cdp,
      "Topology",
      `Boolean(document.querySelector(".topology-view"))`,
      "topology before large graph update",
    );
    await waitFor(
      cdp,
      `document.querySelectorAll("g.topology-node[role=option]").length === 3`,
      "topology graph before large update",
    );
    const fillerNodes = Array.from({ length: 120 }, (_, index) => {
      const nodeId = `node_F${String(index).padStart(42, "0")}`;
      return {
        node_id: nodeId,
        display_name: `Filler node ${String(index).padStart(3, "0")}`,
        health: "healthy",
        is_self: false,
        capabilities: ["file"],
        networks: [
          {
            network_id: index % 2 === 0 ? "home" : "studio",
            health: "healthy",
            presence: presence("online"),
            role: "member",
            status: "active",
            capabilities: ["file"],
          },
        ],
      };
    });
    const totalAfterFillers = state.topology.nodes.length + fillerNodes.length;
    state.topology.nodes.push(...fillerNodes);
    refreshBoundaryCounts(state.topology);
    await evaluate(cdp, `window.__memberE2eNoReload = true`);
    bumpAuthority(state);
    fixtureServer.broadcastSnapshot();
    await waitFor(
      cdp,
      `document.querySelector(".topology-filter-count")?.textContent?.trim() === ${JSON.stringify(
        `Showing ${totalAfterFillers} of ${totalAfterFillers} nodes`,
      )}`,
      "large topology incremental update",
      12_000,
    );
    await waitFor(
      cdp,
      `document.querySelector(".topology-canvas")?.getAttribute("data-layout-pending") === "false" &&
       document.querySelectorAll("g.topology-node[data-node-id]").length > 0`,
      "settled large graph",
      12_000,
    );
    const graphRender = await evaluate(
      cdp,
      `(() => ({
        rendered: Number(document.querySelector(".topology-canvas")?.getAttribute("data-rendered-nodes")),
        dom: document.querySelectorAll("g.topology-node[data-node-id]").length,
        marker: window.__memberE2eNoReload === true,
        navigationEntries: performance.getEntriesByType("navigation").length,
      }))()`,
    );
    assert.equal(graphRender.marker, true, "large update reloaded the document");
    assert.equal(graphRender.navigationEntries, 1);
    assert.ok(graphRender.dom <= 80, `large graph rendered ${graphRender.dom} node elements`);
    const renderedFillerIds = await evaluate(
      cdp,
      `[...document.querySelectorAll("g.topology-node[data-node-id]")].map(
        (node) => node.getAttribute("data-node-id")
      ).filter((nodeId) => nodeId?.startsWith("node_F"))`,
    );
    assert.ok(renderedFillerIds.length >= 2, "large graph has fewer than two rendered fillers");
    const [removedNodeId, survivorNodeId] = renderedFillerIds;
    const removedNode = state.topology.nodes.find((node) => node.node_id === removedNodeId);
    const survivorNode = state.topology.nodes.find((node) => node.node_id === survivorNodeId);
    assert.ok(removedNode && survivorNode, "reconnect fixture nodes are missing");
    const removedNodeName = removedNode.display_name;
    const survivorNodeName = "Authority filler after reconnect";
    const snapshotRequestsBeforeReconnect = fixtureServer.snapshotRequestCount();
    const manifestRequestsBeforeReconnect = fixtureServer.manifestRequestCount();
    const websocketConnectionsBeforeReconnect = fixtureServer.websocketConnectionCount();
    const reconnectsBefore = state.reconnects;
    assert.ok(
      fixtureServer.disconnectClients() >= 1,
      "member fixture has no connected WebSocket client to disconnect",
    );
    state.topology.nodes = state.topology.nodes.filter(
      (node) => node.node_id !== removedNodeId,
    );
    state.topology.connections = state.topology.connections.filter(
      (connection) => connection.from !== removedNodeId && connection.to !== removedNodeId,
    );
    survivorNode.display_name = survivorNodeName;
    refreshBoundaryCounts(state.topology);
    bumpAuthority(state);
    await waitForCondition(
      () => fixtureServer.snapshotRequestCount() > snapshotRequestsBeforeReconnect,
      "authoritative snapshot resync after reconnect",
      15_000,
    );
    await waitForCondition(
      () => fixtureServer.manifestRequestCount() > manifestRequestsBeforeReconnect,
      "authoritative manifest resync after reconnect",
      15_000,
    );
    await waitForCondition(
      () => fixtureServer.websocketConnectionCount() > websocketConnectionsBeforeReconnect,
      "automatic WebSocket reconnect",
      15_000,
    );
    await waitForCondition(
      () => state.reconnects > reconnectsBefore,
      "reconnect counter increment",
      5_000,
    );
    await selectTopologySearch(cdp, removedNodeName);
    await waitFor(
      cdp,
      `document.querySelector(".topology-filter-count")?.textContent?.trim() === ${JSON.stringify(
        `Showing 0 of ${totalAfterFillers - 1} nodes`,
      )}`,
      "stale node removed after reconnect",
      12_000,
    );
    await sleep(500);
    assert.equal(
      await evaluate(
        cdp,
        `document.querySelector(".topology-filter-count")?.textContent?.trim()`,
      ),
      `Showing 0 of ${totalAfterFillers - 1} nodes`,
      "stale node reappeared after reconnect",
    );
    await selectTopologySearch(cdp, survivorNodeName);
    await waitFor(
      cdp,
      `document.querySelector(".topology-filter-count")?.textContent?.trim() === "Showing 1 of ${totalAfterFillers - 1} nodes" &&
       [...document.querySelectorAll("g.topology-node[data-node-id]")].some(
         (node) => node.getAttribute("data-node-id") === ${JSON.stringify(survivorNodeId)} &&
           node.getAttribute("aria-label")?.includes(${JSON.stringify(survivorNodeName)})
       )`,
      "renamed survivor after reconnect",
      12_000,
    );
    assert.equal(
      await evaluate(cdp, `window.__memberE2eNoReload === true`),
      true,
      "reconnect resync reloaded the document",
    );
    checks.push("large graph updates incrementally without reloading and bounds rendered node DOM");
    checks.push("disconnect resync refetches authority, removes stale nodes and applies renames after reconnect");

    const browserVersion = await cdp.send("Browser.getVersion");
    const commandCounts = Object.fromEntries(
      ["create_invitation", "list_network_members", "preview_member_removal", "change_member_lifecycle"].map(
        (command) => [command, fixtureServer.commandCount(command)],
      ),
    );
    const scriptSource = await readFile(fileURLToPath(import.meta.url));
    const report = {
      schema_version: 1,
      kind: "topology-member-e2e",
      generated_at: new Date().toISOString(),
      commit: {
        sha: gitValue(["rev-parse", "HEAD"]),
        branch: gitValue(["branch", "--show-current"]),
      },
      environment: {
        chrome: browserVersion.product,
        platform: process.platform,
        node: process.version,
        viewports: { desktop: DESKTOP_VIEWPORT, mobile: MOBILE_VIEWPORT },
        locale: "en-US",
        timezone: "UTC",
        reduced_motion: "reduce",
      },
      script: {
        path: "webui/scripts/topology-member-e2e.mjs",
        sha256: createHash("sha256").update(scriptSource).digest("hex"),
      },
      invitation: {
        format_valid: true,
        code_recorded: false,
        payload: invitationCommand.payload,
      },
      topology: {
        initial_revision: 12,
        approved_revision: 13,
        degraded_revision: 14,
        removed_revision: 15,
        final_revision: state.topology.revision,
        initial_nodes: 3,
        large_graph_nodes: totalAfterFillers,
        nodes_after_reconnect: state.topology.nodes.length,
        rendered_graph_nodes_after_large_update: graphRender.rendered,
        rendered_graph_dom_after_large_update: graphRender.dom,
      },
      lifecycle: {
        approve: {
          payload: approveCommand.payload,
          outcome: approveOutcome.data,
        },
        remove: {
          payload: removeCommand.payload,
          outcome: removeOutcome.data,
        },
      },
      member_directory: {
        preview_payload: previewCommand.payload,
        impact_items: 6,
        bob_removed_from_home: true,
      },
      group_policy: {
        source: "authoritative_snapshot_and_member_directory",
        policy_id: state.groupPolicy.policy_id,
        final_revision: state.groupPolicy.revision,
        before: {
          bob_group: "file-readers",
          bob_capabilities: ["file"],
          viewer_role: "member",
          can_manage_members: true,
        },
        restricted: {
          bob_group: "relay-operators",
          bob_capabilities: ["relay"],
          viewer_role: "member",
          can_manage_members: false,
        },
        restored: {
          bob_group: "file-readers",
          bob_capabilities: ["file"],
          viewer_role: "owner",
          can_manage_members: true,
        },
        directory_refreshes: groupPolicyDirectoryRequests,
        lifecycle_actions_restored: true,
      },
      audit_trace: {
        remove_response_audit_id: removeOutcome.data.audit_id,
        overview_activity_audit_id: REMOVE_AUDIT_ID,
        overview_activity_contains_command: /change_member_lifecycle/.test(activityText),
      },
      reconnect: {
        snapshot_requests:
          fixtureServer.snapshotRequestCount() - snapshotRequestsBeforeReconnect,
        manifest_requests:
          fixtureServer.manifestRequestCount() - manifestRequestsBeforeReconnect,
        websocket_connections:
          fixtureServer.websocketConnectionCount() - websocketConnectionsBeforeReconnect,
        authority_nodes_after: state.topology.nodes.length,
        stale_node_removed: true,
        renamed_node_applied: true,
        document_reloaded: false,
      },
      commands: state.commands,
      command_counts: commandCounts,
      checks,
      check_count: checks.length,
    };
    report.commands = report.commands.filter(
      (entry) => entry.command !== "create_invitation",
    );
    report.commands.unshift(invitationCommand);
    const invitationReport = JSON.stringify(report);
    assert.equal(invitationReport.includes("fkinv_"), false, "invitation code leaked into report");
    await writeFile(
      join(REPORT_ROOT, "report.json"),
      `${JSON.stringify(report, null, 2)}\n`,
    );
    process.stdout.write(`topology member E2E passed (${checks.length} checks)\n`);
    for (const check of checks) process.stdout.write(`- ${check}\n`);
    process.stdout.write(`${join(REPORT_ROOT, "report.json")}\n`);
  } catch (error) {
    if (chromeStderr) process.stderr.write(`Chrome stderr:\n${chromeStderr}\n`);
    throw error;
  } finally {
    cdp?.close();
    await stopChild(chrome);
    await fixtureServer?.close();
    await rm(workspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error}\n`);
  process.exitCode = 1;
});
