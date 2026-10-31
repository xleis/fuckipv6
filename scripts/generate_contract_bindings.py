#!/usr/bin/env python3
"""Generate TypeScript and Android bindings from checked-in JSON Schemas.

Rust derives the JSON Schemas; this script only projects those schemas into
consumer-language types. ``--check`` renders in memory and fails when any
checked-in binding has drifted.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
SCHEMA_DIR = ROOT / "contracts" / "schema"
TS_OUTPUT = ROOT / "contracts" / "typescript" / "node-contract-v1.ts"
JAVA_OUTPUT = (
    ROOT
    / "contracts"
    / "android"
    / "src"
    / "main"
    / "java"
    / "cc"
    / "xlei"
    / "fuckipv6"
    / "contract"
    / "NodeContractV1.java"
)
SCHEMA_VERSION = 1
ROOT_SCHEMAS = {
    "node-command-v1.schema.json": "NodeCommand",
    "node-command-request-v1.schema.json": "NodeCommandRequest",
    "node-command-result-v1.schema.json": "NodeCommandResult",
    "node-event-v1.schema.json": "NodeEvent",
    "node-ui-manifest-v1.schema.json": "NodeUiManifest",
    "node-http-v1.schema.json": "NodeHttpContract",
    "node-snapshot-v1.schema.json": "NodeSnapshot",
}
ROOT_METADATA = {"$schema", "$id", "title", "x-fuckipv6-contract-version", "x-fuckipv6-rust-source"}


class ContractError(RuntimeError):
    pass


@dataclass(frozen=True)
class Contract:
    schemas: dict[str, dict[str, Any]]
    definitions: dict[str, dict[str, Any]]
    command_names: tuple[str, ...]
    action_command_names: tuple[str, ...]
    string_enums: dict[str, tuple[str, ...]]


def stripped(value: dict[str, Any]) -> dict[str, Any]:
    return {key: child for key, child in value.items() if key not in ROOT_METADATA | {"$defs"}}


def load_contract() -> Contract:
    schemas: dict[str, dict[str, Any]] = {}
    definitions: dict[str, dict[str, Any]] = {}
    for filename, type_name in ROOT_SCHEMAS.items():
        path = SCHEMA_DIR / filename
        try:
            schema = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ContractError(f"cannot load {path}: {error}") from error
        if schema.get("x-fuckipv6-contract-version") != SCHEMA_VERSION:
            raise ContractError(f"{filename}: unsupported contract version")
        schemas[filename] = schema
        for name, definition in schema.get("$defs", {}).items():
            previous = definitions.get(name)
            if previous is not None and previous != definition:
                raise ContractError(f"definition {name} differs between generated schemas")
            definitions[name] = definition
        root = stripped(schema)
        previous = definitions.get(type_name)
        if previous is not None and previous != root:
            raise ContractError(f"root type {type_name} conflicts with a referenced definition")
        definitions[type_name] = root

    command_document = schemas["node-command-v1.schema.json"]
    command_names = extract_command_constants(command_document)
    if not command_names:
        raise ContractError("command schema contains no command constants")

    action_names: set[str] = set()
    collect_property_enums(
        schemas["node-ui-manifest-v1.schema.json"], "command", action_names
    )
    action_names &= set(command_names)
    if "get_ui_manifest" in action_names:
        raise ContractError("bootstrap get_ui_manifest must not be a manifest action")

    string_enums: dict[str, tuple[str, ...]] = {}
    for name, definition in definitions.items():
        values = string_enum_values(definition)
        if values:
            string_enums[name] = values

    return Contract(
        schemas=schemas,
        definitions=definitions,
        command_names=tuple(command_names),
        action_command_names=tuple(sorted(action_names)),
        string_enums=string_enums,
    )


def extract_command_constants(schema: dict[str, Any]) -> list[str]:
    values: list[str] = []

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            command = value.get("command")
            if isinstance(command, dict) and isinstance(command.get("const"), str):
                values.append(command["const"])
            for child in value.values():
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    visit(schema)
    return sorted(set(values))


def collect_property_enums(value: Any, property_name: str, output: set[str]) -> None:
    if isinstance(value, dict):
        properties = value.get("properties")
        if isinstance(properties, dict):
            prop = properties.get(property_name)
            if isinstance(prop, dict) and isinstance(prop.get("enum"), list):
                output.update(item for item in prop["enum"] if isinstance(item, str))
        for child in value.values():
            collect_property_enums(child, property_name, output)
    elif isinstance(value, list):
        for child in value:
            collect_property_enums(child, property_name, output)


def string_enum_values(schema: dict[str, Any]) -> tuple[str, ...]:
    values = schema.get("enum")
    if isinstance(values, list) and values and all(isinstance(item, str) for item in values):
        return tuple(values)
    variants = schema.get("oneOf")
    if isinstance(variants, list):
        constants = [
            variant.get("const")
            for variant in variants
            if isinstance(variant, dict) and isinstance(variant.get("const"), str)
        ]
        if constants and len(constants) == len(variants):
            return tuple(constants)
    return ()


class TypeScriptGenerator:
    def __init__(self, contract: Contract):
        self.contract = contract

    def render(self) -> str:
        aliases = []
        for name in sorted(self.contract.definitions):
            ts_type = self.ts_type(self.contract.definitions[name], depth=0)
            aliases.append(f"export type {name} = {ts_type};")

        command_names = json.dumps(list(self.contract.command_names), ensure_ascii=False)
        action_names = json.dumps(list(self.contract.action_command_names), ensure_ascii=False)
        aliases.insert(
            0,
            "export const NODE_CONTRACT_SCHEMA_VERSION = 1 as const;\n"
            f"export const NODE_COMMAND_KINDS = {command_names} as const;\n"
            "export type NodeCommandKindWire = (typeof NODE_COMMAND_KINDS)[number];\n"
            f"export const NODE_MANIFEST_ACTION_COMMAND_KINDS = {action_names} as const;\n"
            "export type NodeManifestActionCommandKind =\n"
            "  (typeof NODE_MANIFEST_ACTION_COMMAND_KINDS)[number];",
        )
        client = TYPESCRIPT_CLIENT
        return (
            "// Generated from contracts/schema/*.schema.json. Do not edit by hand.\n"
            "// Regenerate with scripts/generate-contracts.sh.\n\n"
            + "\n\n".join(aliases)
            + "\n\n"
            + client
        )

    def ts_type(self, schema: Any, depth: int) -> str:
        if isinstance(schema, bool):
            return "unknown" if schema else "never"
        if not isinstance(schema, dict):
            return "unknown"

        reference = schema.get("$ref")
        if isinstance(reference, str):
            prefix = "#/$defs/"
            if not reference.startswith(prefix):
                raise ContractError(f"unsupported schema reference {reference}")
            return reference[len(prefix) :]

        if "const" in schema:
            return json.dumps(schema["const"], ensure_ascii=False)

        if isinstance(schema.get("enum"), list):
            return " | ".join(
                json.dumps(value, ensure_ascii=False) for value in schema["enum"]
            ) or "never"

        for keyword in ("oneOf", "anyOf"):
            variants = schema.get(keyword)
            if isinstance(variants, list):
                rendered = []
                for variant in variants:
                    value = self.ts_type(variant, depth + 1)
                    if value not in rendered:
                        rendered.append(value)
                return " | ".join(rendered) if rendered else "never"

        if isinstance(schema.get("allOf"), list):
            return " & ".join(self.ts_type(item, depth + 1) for item in schema["allOf"])

        schema_type = schema.get("type")
        if isinstance(schema_type, list):
            return " | ".join(
                self.ts_type({**schema, "type": item}, depth + 1) for item in schema_type
            )

        if schema_type == "null":
            return "null"
        if schema_type in {"integer", "number"}:
            return "number"
        if schema_type == "boolean":
            return "boolean"
        if schema_type == "string":
            return "string"
        if schema_type == "array":
            prefix_items = schema.get("prefixItems")
            if isinstance(prefix_items, list):
                return "[" + ", ".join(self.ts_type(item, depth + 1) for item in prefix_items) + "]"
            return f"Array<{self.ts_type(schema.get('items', {}), depth + 1)}>"
        if schema_type == "object" or "properties" in schema or "additionalProperties" in schema:
            properties = schema.get("properties")
            if isinstance(properties, dict) and properties:
                required = set(schema.get("required", []))
                lines = ["{"]
                for name, property_schema in properties.items():
                    optional = "?" if name not in required else ""
                    lines.append(
                        f"  {json.dumps(name)}{optional}: {self.ts_type(property_schema, depth + 1)};"
                    )
                lines.append("}")
                return "\n".join(lines)
            additional = schema.get("additionalProperties")
            if isinstance(additional, dict):
                return f"Record<string, {self.ts_type(additional, depth + 1)}>"
            if additional is False:
                return "Record<string, never>"
            return "Record<string, unknown>"
        return "unknown"


def indent_block(value: str, spaces: int) -> str:
    prefix = " " * spaces
    return "\n".join(prefix + line if line else line for line in value.splitlines())


def java_constant(value: str) -> str:
    name = re.sub(r"[^A-Za-z0-9]+", "_", value).strip("_").upper()
    if not name:
        raise ContractError(f"cannot derive Java enum name from {value!r}")
    if name[0].isdigit():
        name = "VALUE_" + name
    return name


def render_java(contract: Contract) -> str:
    enums: dict[str, tuple[str, ...]] = dict(contract.string_enums)
    enums["NodeCommandKind"] = contract.command_names
    lines = [
        "// Generated from contracts/schema/*.schema.json. Do not edit by hand.",
        "// Regenerate with scripts/generate-contracts.sh.",
        "package cc.xlei.fuckipv6.contract;",
        "",
        "/** Versioned Android-safe bindings for node wire enum values. */",
        "public final class NodeContractV1 {",
        "    public static final int SCHEMA_VERSION = 1;",
        "",
    ]
    for name in sorted(enums):
        values = enums[name]
        seen: set[str] = set()
        lines.append(f"    public enum {name} {{")
        entries = []
        for value in values:
            constant = java_constant(value)
            if constant in seen:
                raise ContractError(f"duplicate Java enum constant {name}.{constant}")
            seen.add(constant)
            entries.append(f'        {constant}("{value}")')
        lines.append(",\n".join(entries) + ";")
        lines.extend(
            [
                "",
                "        private final String wireName;",
                "",
                f"        {name}(String wireName) {{",
                "            this.wireName = wireName;",
                "        }",
                "",
                "        public String wireName() {",
                "            return wireName;",
                "        }",
                "",
                f"        public static {name} fromWireName(String value) {{",
                "            for (" + name + " candidate : values()) {",
                "                if (candidate.wireName.equals(value)) {",
                "                    return candidate;",
                "                }",
                "            }",
                '            throw new IllegalArgumentException("unknown ' + name + ' wire value: " + value);',
                "        }",
                "    }",
                "",
            ]
        )

    action_values = " ||\n".join(
        f'            command == NodeCommandKind.{java_constant(value)}'
        for value in contract.action_command_names
    )
    lines.extend(
        [
            "    /** Parsed command identity; payload JSON remains owned by the platform JSON parser. */",
            "    public static final class CommandEnvelope {",
            "        public final long requestId;",
            "        public final NodeCommandKind command;",
            "",
            "        private CommandEnvelope(long requestId, NodeCommandKind command) {",
            "            this.requestId = requestId;",
            "            this.command = command;",
            "        }",
            "    }",
            "",
            "    private NodeContractV1() {}",
            "",
            "    /** Validate the version, request id and canonical command name before dispatch. */",
            "    public static CommandEnvelope commandEnvelope(",
            "            long schemaVersion, long requestId, String commandName) {",
            "        if (schemaVersion != SCHEMA_VERSION) {",
            '            throw new IllegalArgumentException("unsupported node contract version: " + schemaVersion);',
            "        }",
            "        if (requestId < 0) {",
            '            throw new IllegalArgumentException("request id must be non-negative");',
            "        }",
            "        NodeCommandKind command = NodeCommandKind.fromWireName(commandName);",
            "        if (!isManifestAction(command)) {",
            '            throw new IllegalArgumentException("command is not a user-visible manifest action: " + commandName);',
            "        }",
            "        return new CommandEnvelope(requestId, command);",
            "    }",
            "",
            "    public static boolean isManifestAction(NodeCommandKind command) {",
            "        return " + action_values + ";",
            "    }",
            "}",
            "",
        ]
    )
    return "\n".join(lines)


TYPESCRIPT_CLIENT = r'''
export type NodeFetcher = (input: string, init?: RequestInit) => Promise<Response>;

/** Thin transport client. Deserialize once at the edge; do not clone domain logic. */
export class NodeContractV1Client {
  private readonly baseUrl: string;
  private readonly token: string;
  private readonly fetcher: NodeFetcher;

  constructor(
    baseUrl: string,
    token: string,
    fetcher: NodeFetcher = globalThis.fetch.bind(globalThis),
  ) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.token = token;
    this.fetcher = fetcher;
  }

  async loadManifest(): Promise<NodeManifestResponse> {
    return this.request<NodeManifestResponse>(
      `/api/node/manifest?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
    );
  }

  async loadSnapshot(): Promise<NodeSnapshotResponse> {
    return this.request<NodeSnapshotResponse>(
      `/api/node/snapshot?schema_version=${NODE_CONTRACT_SCHEMA_VERSION}`,
    );
  }

  async sendCommand(request: NodeCommandRequest): Promise<NodeCommandHttpResponse> {
    const response = await this.request<NodeCommandHttpResponse>(
      "/api/node/commands",
      {
        method: "POST",
        body: JSON.stringify({
          schema_version: NODE_CONTRACT_SCHEMA_VERSION,
          request,
        } satisfies NodeCommandHttpRequest),
      },
    );
    if (response.result.request_id !== request.request_id) {
      throw new Error(
        `node contract: response request_id ${response.result.request_id} does not match ${request.request_id}`,
      );
    }
    return response;
  }

  private async request<T extends { schema_version: number }>(
    path: string,
    init: RequestInit = {},
  ): Promise<T> {
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Bearer ${this.token}`);
    if (init.body !== undefined) {
      headers.set("Content-Type", "application/json");
    }
    const response = await this.fetcher(`${this.baseUrl}${path}`, { ...init, headers });
    const body: unknown = await response.json();
    if (!isRecord(body) || body.schema_version !== NODE_CONTRACT_SCHEMA_VERSION) {
      throw new Error("node contract: schema version mismatch");
    }
    if (!response.ok) {
      throw new NodeContractHttpError(response.status, body as unknown as NodeHttpErrorResponse);
    }
    return body as T;
  }
}

export class NodeContractHttpError extends Error {
  constructor(
    readonly status: number,
    readonly payload: NodeHttpErrorResponse,
  ) {
    super(payload.message || `node HTTP error ${status}`);
    this.name = "NodeContractHttpError";
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
'''


def rendered_bindings() -> dict[Path, str]:
    contract = load_contract()
    return {
        TS_OUTPUT: TypeScriptGenerator(contract).render(),
        JAVA_OUTPUT: render_java(contract),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail instead of writing when checked-in bindings have drifted",
    )
    args = parser.parse_args()

    try:
        bindings = rendered_bindings()
        stale = []
        for path, contents in bindings.items():
            if args.check:
                try:
                    actual = path.read_text(encoding="utf-8")
                except OSError:
                    actual = None
                if actual != contents:
                    stale.append(path.relative_to(ROOT))
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(contents, encoding="utf-8")
        if stale:
            print(
                "generate-contract-bindings: stale generated bindings: "
                + ", ".join(map(str, stale)),
                file=sys.stderr,
            )
            return 1
        state = "verified" if args.check else "generated"
        print(f"generate-contract-bindings: {state} {len(bindings)} artifact(s)")
        return 0
    except ContractError as error:
        print(f"generate-contract-bindings: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
