#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const compiledPath = path.resolve(
  process.argv[2] ?? path.join(root, "target/contract-typescript/node-contract-v1.js"),
);
const binding = await import(pathToFileURL(compiledPath).href);

function commandConstants(value, output = new Set()) {
  if (Array.isArray(value)) {
    for (const item of value) commandConstants(item, output);
  } else if (value && typeof value === "object") {
    if (value.command && typeof value.command === "object" && typeof value.command.const === "string") {
      output.add(value.command.const);
    }
    for (const child of Object.values(value)) commandConstants(child, output);
  }
  return output;
}

function propertyEnums(value, propertyName, output = new Set()) {
  if (Array.isArray(value)) {
    for (const item of value) propertyEnums(item, propertyName, output);
  } else if (value && typeof value === "object") {
    const property = value.properties?.[propertyName];
    if (property && Array.isArray(property.enum)) {
      for (const item of property.enum) {
        if (typeof item === "string") output.add(item);
      }
    }
    for (const child of Object.values(value)) propertyEnums(child, propertyName, output);
  }
  return output;
}

const commandSchema = JSON.parse(
  await readFile(path.join(root, "contracts/schema/node-command-v1.schema.json"), "utf8"),
);
const manifestSchema = JSON.parse(
  await readFile(path.join(root, "contracts/schema/node-ui-manifest-v1.schema.json"), "utf8"),
);
const schemaCommands = [...commandConstants(commandSchema)].sort();
const schemaActions = [...propertyEnums(manifestSchema, "command")].sort();

assert.equal(binding.NODE_CONTRACT_SCHEMA_VERSION, 1, "schema version");
assert.deepEqual([...binding.NODE_COMMAND_KINDS].sort(), schemaCommands, "command constants");
assert.deepEqual(
  [...binding.NODE_MANIFEST_ACTION_COMMAND_KINDS].sort(),
  schemaActions,
  "manifest action constants",
);
assert.ok(!binding.NODE_MANIFEST_ACTION_COMMAND_KINDS.includes("get_ui_manifest"));

const requests = [];
const completedResponse = (requestId) => ({
  schema_version: 1,
  result: {
    request_id: requestId,
    outcome: { status: "completed", data: { message: "ok" } },
  },
});
const fetcher = async (input, init) => {
  requests.push({ input: String(input), init });
  const body = JSON.parse(String(init?.body));
  return new Response(JSON.stringify(completedResponse(body.request.request_id)), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
};
const client = new binding.NodeContractV1Client("https://node.example/", "secret-token", fetcher);
const request = { request_id: 9, command: { command: "get_snapshot" } };
const response = await client.sendCommand(request);
assert.equal(response.result.request_id, 9);
assert.equal(requests.length, 1);
assert.equal(requests[0].input, "https://node.example/api/node/commands");
assert.equal(requests[0].init.headers.get("Authorization"), "Bearer secret-token");
assert.deepEqual(JSON.parse(String(requests[0].init.body)), {
  schema_version: 1,
  request,
});

await assert.rejects(
  new binding.NodeContractV1Client("https://node.example", "token", async () =>
    new Response(JSON.stringify({ schema_version: 2 }), { status: 200 }),
  ).loadSnapshot(),
  /schema version mismatch/,
);

await assert.rejects(
  new binding.NodeContractV1Client("https://node.example", "token", async () =>
    new Response(JSON.stringify(completedResponse(10)), { status: 200 }),
  ).sendCommand(request),
  /does not match/,
);

console.log("test_contract_bindings: passed");
