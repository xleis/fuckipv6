#!/usr/bin/env node
/**
 * Fixture server shared by the browser harnesses.
 *
 * Serves the built WebUI bundle, the shared assets exported from Rust, and a
 * deterministic API surface built from the contract fixtures, so a browser can
 * drive real user flows without a running node.
 */
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { join } from "node:path";

const FIXED_SERVER_TIME = "2026-01-01T00:00:05Z";
const WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

function jsonResponse(response, value) {
  response.writeHead(200, {
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

export async function startFixtureServer({ manifest, fixture, distRoot, sharedAssets }) {
  const appHtml = await readFile(join(distRoot, "index.html"));
  const appJs = await readFile(join(distRoot, "assets/app.js"));
  const appCss = await readFile(join(distRoot, "assets/app.css"));
  const webuiCss = await readFile(join(sharedAssets, "webui.css"));
  const i18nJs = await readFile(join(sharedAssets, "i18n.js"));
  const sockets = new Set();

  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    switch (url.pathname) {
      case "/":
        fileResponse(response, appHtml, "text/html; charset=utf-8");
        return;
      case "/assets/app.js":
        fileResponse(response, appJs, "text/javascript; charset=utf-8");
        return;
      case "/assets/app.css":
        fileResponse(response, appCss, "text/css; charset=utf-8");
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
        jsonResponse(response, {
          schema_version: 1,
          request_id: 1,
          server_time: FIXED_SERVER_TIME,
          manifest,
        });
        return;
      case "/api/v1/snapshot":
        jsonResponse(response, {
          schema_version: 1,
          request_id: 1,
          server_time: FIXED_SERVER_TIME,
          snapshot: fixture.snapshot,
        });
        return;
      default:
        if (url.pathname.startsWith("/assets/")) {
          const assetName = url.pathname.slice("/assets/".length);
          if (
            assetName.length > 0 &&
            !assetName.includes("/") &&
            !assetName.includes("\\") &&
            assetName !== "." &&
            assetName !== ".."
          ) {
            readFile(join(distRoot, "assets", assetName))
              .then((body) => {
                const contentType = assetName.endsWith(".js")
                  ? "text/javascript; charset=utf-8"
                  : "application/octet-stream";
                fileResponse(response, body, contentType);
              })
              .catch(() => {
                response.writeHead(404, {
                  "content-type": "text/plain; charset=utf-8",
                });
                response.end("not found");
              });
            return;
          }
        }
        response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
        response.end("not found");
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
  });

  await new Promise((resolvePromise, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolvePromise);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("fixture server did not bind TCP");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolvePromise) => server.close(resolvePromise));
    },
  };
}
