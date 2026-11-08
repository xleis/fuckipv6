import type {
  NodeSnapshot,
  NodeUiAction,
} from "../../contracts/typescript/node-contract-v1";
import {
  blockReasonText,
  executionBlockReason,
  label,
} from "./contract/invariants";
import { tr } from "./i18n";

export type SecurityFact = {
  id: string;
  label: string;
  value: string;
  detail?: string;
  tone?: "neutral" | "warn";
};

export type PolicyWriteAccess = {
  canModify: boolean;
  value: string;
  reason: string;
  requiredPermission: string;
  requestPath: string;
};

export type SecurityProjection = {
  facts: SecurityFact[];
  policyWriteAccess: PolicyWriteAccess;
};

function timestamp(locale: Locale, unixMs: number): string {
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(unixMs));
}

function validity(
  locale: Locale,
  issuedAtUnixMs: number,
  expiresAtUnixMs: number,
): string {
  return tr(locale, "settings.security.validity", {
    issued: timestamp(locale, issuedAtUnixMs),
    expires: timestamp(locale, expiresAtUnixMs),
  });
}

function join(locale: Locale, values: string[]): string {
  return values.length
    ? values.join(label(locale, "common.list_separator"))
    : label(locale, "common.none");
}

export function projectSecurityState(
  snapshot: NodeSnapshot,
  locale: Locale,
  actions: NodeUiAction[],
  selectedNetwork: string,
): SecurityProjection {
  const permissions = snapshot.permissions?.permissions ?? [];
  const accessControl = snapshot.access_control ?? undefined;
  const localPolicy = accessControl?.local_policy;
  const networkAcls = accessControl?.network_acls ?? undefined;
  const privacyPolicies =
    accessControl?.field_privacy_policies ?? undefined;
  const memberships = snapshot.memberships ?? undefined;
  const notExposed = label(locale, "settings.value.not_exposed");
  const none = label(locale, "common.none");
  const unknown = label(locale, "common.unknown");
  const notAvailable = label(locale, "common.not_available");
  const separator = label(locale, "common.list_separator");
  const networkName = (networkId: string): string =>
    memberships?.find((membership) => membership.network_id === networkId)
      ?.network_name ?? networkId;

  const applyPolicy = actions.find((action) => action.id === "apply_policy");
  const policyBlockReason = applyPolicy
    ? executionBlockReason(applyPolicy, snapshot, selectedNetwork)
    : "backend_unavailable";
  const requiredPermission = label(
    locale,
    "choice.permission.manage_networks",
  );
  const requestPath = label(locale, "settings.policy_access.request_path");
  const policyReason = policyBlockReason
    ? blockReasonText(policyBlockReason, locale)
    : "";
  const policyWriteAccess: PolicyWriteAccess = {
    canModify: policyBlockReason === null,
    value: label(
      locale,
      policyBlockReason
        ? "settings.policy_access.read_only"
        : "settings.policy_access.writable",
    ),
    reason: policyReason,
    requiredPermission,
    requestPath,
  };

  const facts: SecurityFact[] = [
    {
      id: "owner_identity",
      label: label(locale, "settings.row.owner_identity"),
      value: snapshot.identity?.display_name?.trim() || unknown,
    },
    {
      id: "node_id",
      label: label(locale, "settings.row.node_id"),
      value: snapshot.identity?.node_id ?? notAvailable,
    },
    {
      id: "public_key_fingerprint",
      label: label(locale, "settings.row.public_key_fingerprint"),
      value: snapshot.identity?.public_key_fingerprint ?? notAvailable,
    },
    {
      id: "access_model",
      label: label(locale, "settings.row.access_model"),
      value: label(
        locale,
        `choice.access_model.${snapshot.permissions?.access_model ?? "trusted_control_plane"}`,
      ),
    },
    {
      id: "granted_permissions",
      label: label(locale, "settings.row.granted_permissions"),
      value: permissions.length
        ? permissions
            .map((permission) => label(locale, `choice.permission.${permission}`))
            .join(separator)
        : none,
    },
    {
      id: "local_policy_revision",
      label: label(locale, "settings.row.local_policy_revision"),
      value:
        accessControl === undefined
          ? notExposed
          : localPolicy
            ? tr(locale, "settings.security.revision", {
                revision: localPolicy.revision,
              })
            : none,
    },
    {
      id: "local_policy_validity",
      label: label(locale, "settings.row.local_policy_validity"),
      value: localPolicy
        ? validity(
            locale,
            localPolicy.issued_at_unix_ms,
            localPolicy.expires_at_unix_ms,
          )
        : accessControl === undefined
          ? notExposed
          : none,
    },
    {
      id: "local_policy_rules",
      label: label(locale, "settings.row.local_policy_rules"),
      value: localPolicy
        ? tr(locale, "settings.security.rule_count", {
            count: localPolicy.rules?.length ?? 0,
          })
        : accessControl === undefined
          ? notExposed
          : none,
    },
    {
      id: "local_policy_vetoes",
      label: label(locale, "settings.row.local_policy_vetoes"),
      value: localPolicy
        ? tr(locale, "settings.security.veto_count", {
            count: localPolicy.contribution_vetoes?.length ?? 0,
          })
        : accessControl === undefined
          ? notExposed
          : none,
    },
    {
      id: "network_acls",
      label: label(locale, "settings.row.network_acls"),
      value:
        networkAcls === undefined
          ? notExposed
          : join(
              locale,
              (networkAcls ?? []).map((acl) =>
                tr(locale, "settings.security.network_acl", {
                  network: networkName(acl.network_id),
                  revision: acl.revision,
                  rules: acl.rules?.length ?? 0,
                  issued: timestamp(locale, acl.issued_at_unix_ms),
                  expires: timestamp(locale, acl.expires_at_unix_ms),
                }),
              ),
            ),
    },
    {
      id: "field_privacy_policies",
      label: label(locale, "settings.row.field_privacy_policies"),
      value:
        privacyPolicies === undefined
          ? notExposed
          : join(
              locale,
              (privacyPolicies ?? []).map((policy) =>
                tr(locale, "settings.security.field_privacy", {
                  network: policy.network_id
                    ? networkName(policy.network_id)
                    : label(locale, "settings.security.node_wide"),
                  revision: policy.revision,
                  rules: policy.rules?.length ?? 0,
                  effective: timestamp(
                    locale,
                    policy.effective_at_unix_ms,
                  ),
                  expires: timestamp(locale, policy.expires_at_unix_ms),
                }),
              ),
            ),
    },
    {
      id: "membership_policy_revisions",
      label: label(locale, "settings.row.membership_policy_revisions"),
      value:
        memberships === undefined
          ? notExposed
          : join(
              locale,
              (memberships ?? []).map((membership) =>
                tr(locale, "settings.security.membership_policy", {
                  network: membership.network_name,
                  local: membership.local_policy_revision,
                  network_policy:
                    membership.network_policy_revision ?? notAvailable,
                  privacy: membership.privacy_policy_revision ?? notAvailable,
                }),
              ),
            ),
    },
    {
      id: "policy_write_access",
      label: label(locale, "settings.row.policy_write_access"),
      value: policyWriteAccess.value,
      detail: [
        policyWriteAccess.reason,
        tr(locale, "settings.policy_access.required_permission", {
          permission: policyWriteAccess.requiredPermission,
        }),
        policyWriteAccess.requestPath,
        tr(locale, "settings.policy_access.command", {
          command: "apply_policy",
        }),
      ]
        .filter(Boolean)
        .join(separator),
      tone: policyWriteAccess.canModify ? "neutral" : "warn",
    },
  ];

  return { facts, policyWriteAccess };
}
