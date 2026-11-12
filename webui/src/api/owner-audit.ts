import type { NodeApiErrorResponse } from "../../../contracts/typescript/node-contract-v1";
import {
  ContractError,
  HttpError,
  requestJson,
} from "./client";

export type OwnerAuditExportFilter = {
  from_unix_ms?: number;
  to_unix_ms?: number;
  network_id?: string;
  actor?: string;
  object_kind?: string;
  object_id?: string;
  resource_prefix?: string;
};

export type OwnerAuditEntry = {
  audit_id: string;
  operation_id?: string | null;
  actor: string;
  network_id?: string | null;
  action: string;
  resource: string;
  outcome: "succeeded" | "failed" | "denied";
  occurred_at_unix_ms: number;
  summary: string;
  details: Record<string, string>;
  sensitive_keys?: string[];
};

export type OwnerAuditChainRecord = {
  schema_version: number;
  sequence: number;
  previous_hash: string;
  entry: OwnerAuditEntry;
  entry_hash: string;
};

export type OwnerAuditExportBundle = {
  manifest: {
    schema_version: number;
    generated_at_unix_ms: number;
    filter: OwnerAuditExportFilter;
    verification: {
      first_sequence?: number | null;
      last_sequence?: number | null;
      last_hash: string;
      record_count: number;
    };
    checkpoint: {
      schema_version: number;
      sequence: number;
      entry_hash: string;
      record_count: number;
    };
  };
  records: OwnerAuditChainRecord[];
};

export type OwnerAuditExport = {
  bundle: OwnerAuditExportBundle;
  filename: string;
};

export type RelayAuditExportRequest = {
  network_id: string;
  from_unix_ms?: number;
  to_unix_ms?: number;
};

export type RelayAuditExportReceipt = {
  filename: string;
  record_count: number;
};

const DEFAULT_AUDIT_EXPORT_FILENAME = "owner-security-audit-export.json";

export function ownerAuditExportFilename(
  contentDisposition: string | null,
): string {
  if (!contentDisposition) return DEFAULT_AUDIT_EXPORT_FILENAME;
  const encoded = /filename\*=UTF-8''([^;]+)/i.exec(contentDisposition)?.[1];
  const quoted = /filename="([^"]+)"/i.exec(contentDisposition)?.[1];
  const bare = /filename=([^;]+)/i.exec(contentDisposition)?.[1];
  let candidate = encoded ?? quoted ?? bare;
  if (!candidate) return DEFAULT_AUDIT_EXPORT_FILENAME;
  try {
    candidate = decodeURIComponent(candidate.trim());
  } catch {
    candidate = candidate.trim();
  }
  const basename = candidate.split(/[\\/]/).pop()?.trim();
  return basename && basename.length > 0
    ? basename
    : DEFAULT_AUDIT_EXPORT_FILENAME;
}

export async function requestOwnerAuditExport(
  filter: OwnerAuditExportFilter,
): Promise<OwnerAuditExport> {
  const { response, payload } = await requestJson<OwnerAuditExportBundle>(
    "/api/v1/owner/security/audit/export",
    {
      method: "POST",
      body: JSON.stringify({ confirmed: true, filter }),
    },
  );
  if (!response.ok) {
    throw new HttpError(
      response.status,
      payload as NodeApiErrorResponse | null,
    );
  }
  if (
    !payload ||
    !Array.isArray(payload.records) ||
    !payload.manifest ||
    typeof payload.manifest !== "object"
  ) {
    throw new ContractError("owner audit export response is invalid");
  }
  return {
    bundle: payload,
    filename: ownerAuditExportFilename(
      response.headers.get("Content-Disposition"),
    ),
  };
}

export function downloadOwnerAuditExport(exported: OwnerAuditExport): void {
  const blob = new Blob([JSON.stringify(exported.bundle, null, 2)], {
    type: "application/json",
  });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = exported.filename;
  anchor.rel = "noopener";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}
