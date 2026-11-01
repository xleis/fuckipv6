import type { NodeSnapshot } from "../../contracts/typescript/node-contract-v1";
import { label } from "./contract/invariants";

export type AdvancedSettingState =
  | "managed"
  | "read_only"
  | "partial"
  | "unsupported";

export type AdvancedSettingRow = {
  id: string;
  title: string;
  detail: string;
  state: AdvancedSettingState;
  keywords: string;
};

export type AdvancedSettingsGroup = {
  id: string;
  title: string;
  rows: AdvancedSettingRow[];
};

function joinValues(values: string[], locale: Locale): string {
  return values.length
    ? values.join(label(locale, "common.list_separator"))
    : label(locale, "common.none");
}

function networkValues(
  snapshot: NodeSnapshot,
  locale: Locale,
  value: (network: NonNullable<NodeSnapshot["configuration"]>["networks"][number]) => string,
): string {
  return joinValues(
    (snapshot.configuration?.networks ?? []).map(
      (network) => `${network.network_name} · ${value(network)}`,
    ),
    locale,
  );
}

function listenPorts(snapshot: NodeSnapshot, locale: Locale): string {
  const live = new Map(
    snapshot.networks.map((network) => [
      network.name,
      network.p2p_listen_addr ?? null,
    ]),
  );
  return networkValues(snapshot, locale, (network) => {
    const address = live.get(network.network_name);
    if (address) return address;
    return network.p2p_listen_port === 0
      ? label(locale, "settings.advanced.port_random")
      : `:${network.p2p_listen_port}`;
  });
}

/**
 * Node-global/control-plane advanced settings inventory. This deliberately
 * describes effective values and unsupported capabilities instead of drawing
 * editable controls before the transactional configuration API exists.
 */
export function advancedSettingsGroups(
  snapshot: NodeSnapshot,
  locale: Locale,
): AdvancedSettingsGroup[] {
  const row = (
    id: string,
    titleKey: string,
    detail: string,
    state: AdvancedSettingState,
    keywords: string,
  ): AdvancedSettingRow => ({
    id,
    title: label(locale, titleKey),
    detail,
    state,
    keywords,
  });

  return [
    {
      id: "network-listeners",
      title: label(locale, "settings.section.network_listeners"),
      rows: [
        row(
          "listen-addresses",
          "settings.row.listen_addresses",
          label(locale, "settings.advanced.listen_addresses.detail"),
          "read_only",
          "listen address bind ipv4 ipv6 wildcard 监听 地址",
        ),
        row(
          "listen-ports",
          "settings.row.listen_ports",
          listenPorts(snapshot, locale),
          "read_only",
          "p2p port udp listen random 端口 监听",
        ),
        row(
          "interface-allowlist",
          "settings.row.interface_allowlist",
          label(locale, "settings.advanced.interface_allowlist.detail"),
          "unsupported",
          "interface allowlist denylist network adapter 接口 白名单 黑名单",
        ),
      ],
    },
    {
      id: "dns-privacy",
      title: label(locale, "settings.section.dns_privacy"),
      rows: [
        row(
          "dns-mode",
          "settings.row.dns_mode",
          label(locale, "settings.advanced.dns_mode.detail"),
          "partial",
          "dns mode resolver doh dot split routing 域名 解析 分流",
        ),
        row(
          "doh-upstreams",
          "settings.row.doh_upstreams",
          label(locale, "settings.advanced.doh_upstreams.detail"),
          "managed",
          "doh dns over https upstream resolver 上游",
        ),
        row(
          "dot",
          "settings.row.dot",
          label(locale, "settings.advanced.dot.detail"),
          "unsupported",
          "dot dns over tls encrypted resolver 加密",
        ),
        row(
          "dns-leak-protection",
          "settings.row.dns_leak_protection",
          label(locale, "settings.advanced.dns_leak.detail"),
          "partial",
          "dns leak protection resolv.conf hijack 泄漏 保护",
        ),
      ],
    },
    {
      id: "transport-limits",
      title: label(locale, "settings.section.transport_limits"),
      rows: [
        row(
          "nat-detection",
          "settings.row.nat_detection",
          label(locale, "settings.advanced.nat_detection.detail"),
          "partial",
          "nat traversal stun mapping 穿透 检测",
        ),
        row(
          "keepalive",
          "settings.row.keepalive",
          networkValues(
            snapshot,
            locale,
            (network) =>
              `${network.p2p_heartbeat_interval_secs} ${label(locale, "settings.advanced.unit_seconds")}`,
          ),
          "read_only",
          "keepalive heartbeat interval connection 心跳 保活",
        ),
        row(
          "connections",
          "settings.row.connections",
          networkValues(
            snapshot,
            locale,
            (network) =>
              `${label(locale, "settings.advanced.connect_timeout")} ${network.p2p_connect_timeout_secs} ${label(locale, "settings.advanced.unit_seconds")}`,
          ),
          "read_only",
          "connection timeout queue concurrency worker batch 连接 并发 队列",
        ),
        row(
          "resource-limits",
          "settings.row.resource_limits",
          label(locale, "settings.advanced.resource_limits.detail"),
          "unsupported",
          "resource limit bandwidth quota connection count target 资源 带宽 配额 连接数 目标",
        ),
      ],
    },
    {
      id: "web-security",
      title: label(locale, "settings.section.web_security"),
      rows: [
        row(
          "webui-listener",
          "settings.row.webui_listener",
          label(locale, "settings.advanced.webui_listener.detail"),
          "managed",
          "webui listen bind origin proxy public 监听 来源 代理 公网",
        ),
        row(
          "webui-session",
          "settings.row.webui_session",
          label(locale, "settings.advanced.webui_session.detail"),
          "read_only",
          "session cookie passkey expiry httponly samesite 会话 登录 有效期",
        ),
        row(
          "webui-rate-limit",
          "settings.row.webui_rate_limit",
          label(locale, "settings.advanced.webui_rate_limit.detail"),
          "read_only",
          "rate limit login mutation brute force 限流 登录 暴力",
        ),
        row(
          "hsts",
          "settings.row.hsts",
          label(locale, "settings.advanced.hsts.detail"),
          "managed",
          "hsts https certificate tls header 证书 安全",
        ),
        row(
          "trusted-proxy",
          "settings.row.trusted_proxy",
          label(locale, "settings.advanced.trusted_proxy.detail"),
          "managed",
          "trusted proxy forwarded origin ip 可信代理 来源",
        ),
      ],
    },
    {
      id: "observability-maintenance",
      title: label(locale, "settings.section.observability_maintenance"),
      rows: [
        row(
          "log-levels",
          "settings.row.log_levels",
          networkValues(
            snapshot,
            locale,
            (network) => network.log_level || label(locale, "common.unknown"),
          ),
          "read_only",
          "log level trace debug info warn error 日志 级别",
        ),
        row(
          "log-retention",
          "settings.row.log_retention",
          label(locale, "settings.advanced.log_retention.detail"),
          "unsupported",
          "log retention remote shipping redaction 日志 保留 远程 脱敏",
        ),
        row(
          "metrics",
          "settings.row.metrics",
          label(locale, "settings.advanced.metrics.detail"),
          "unsupported",
          "metrics prometheus endpoint monitoring 指标 监控 端点",
        ),
        row(
          "updates",
          "settings.row.updates",
          label(locale, "settings.advanced.updates.detail"),
          "unsupported",
          "update channel proxy signature rollback 更新 通道 签名 回滚",
        ),
        row(
          "maintenance-window",
          "settings.row.maintenance_window",
          label(locale, "settings.advanced.maintenance.detail"),
          "unsupported",
          "maintenance window schedule 维护 窗口 计划",
        ),
      ],
    },
  ];
}

export function advancedSettingSearchEntries(
  snapshot: NodeSnapshot,
  locale: Locale,
): Array<AdvancedSettingRow & { group: string }> {
  return advancedSettingsGroups(snapshot, locale).flatMap((group) =>
    group.rows.map((row) => ({ ...row, group: group.title })),
  );
}
