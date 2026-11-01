import type {
  NodeSnapshot,
  NodeUiManifest,
} from "../../contracts/typescript/node-contract-v1";
import { t, tr } from "./i18n";
import { navigationModuleForPage } from "./navigation";
import { advancedSettingSearchEntries } from "./settings";

export type SearchResultKind =
  | "page"
  | "identity"
  | "network"
  | "peer"
  | "setting";

export type SearchResult = {
  id: string;
  kind: SearchResultKind;
  title: string;
  detail: string;
  pageId: string;
  networkName?: string;
};

export function searchElementId(result: SearchResult): string {
  return `search-result-${result.id}`;
}

export function searchKindKey(kind: SearchResultKind): string {
  return `search.kind.${kind}`;
}

/**
 * Search manifest destinations and user-visible snapshot objects locally. The
 * query is never sent to the node or persisted in activity history.
 */
export function searchNodeSnapshot(
  manifest: NodeUiManifest,
  snapshot: NodeSnapshot,
  query: string,
  locale: Locale,
): SearchResult[] {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return [];

  const matches: Array<{
    rank: number;
    index: number;
    result: SearchResult;
  }> = [];
  const push = (result: SearchResult, keywords: string): void => {
    const haystack = `${result.title} ${result.detail} ${result.id} ${keywords}`.toLowerCase();
    const rank = searchMatchRank(haystack, normalized);
    if (rank !== null) {
      matches.push({ rank, index: matches.length, result });
    }
  };

  for (const page of manifest.pages) {
    const module = navigationModuleForPage(page.id);
    push(
      {
        id: `page:${page.id}`,
        kind: "page",
        title: t(locale, page.nav_label_key),
        detail: t(locale, module.labelKey),
        pageId: page.id,
      },
      module.id,
    );
  }

  const identity = snapshot.identity;
  if (identity) {
    const title = identity.display_name?.trim() || identity.node_id;
    push(
      {
        id: `identity:${identity.node_id}`,
        kind: "identity",
        title,
        detail: tr(locale, "search.identity_detail", {
          node_id: identity.node_id,
          fingerprint: identity.public_key_fingerprint,
        }),
        pageId: "identity",
      },
      `${identity.node_id} ${identity.public_key_fingerprint} identity node`,
    );
  }

  for (const network of snapshot.networks) {
    push(
      {
        id: `network:${network.name}`,
        kind: "network",
        title: network.name,
        detail: tr(locale, "search.network_detail", {
          status: t(locale, `status.network.${network.status}`),
          tun: network.tun_name,
          peers: network.peer_count,
        }),
        pageId: "networks",
        networkName: network.name,
      },
      `${network.name} ${network.assigned_ipv4 ?? ""} ${network.assigned_ipv6 ?? ""} network`,
    );
  }

  for (const peer of snapshot.peers) {
    push(
      {
        id: `peer:${peer.network_name}:${peer.peer_id}`,
        kind: "peer",
        title: peer.peer_id,
        detail: tr(locale, "search.peer_detail", {
          network: peer.network_name,
          address: peer.virtual_ipv6,
        }),
        pageId: "peers",
        networkName: peer.network_name,
      },
      `${peer.peer_id} ${peer.network_name} ${peer.virtual_ipv6} ${peer.virtual_ipv4 ?? ""} peer`,
    );
  }

  for (const setting of advancedSettingSearchEntries(snapshot, locale)) {
    push(
      {
        id: `setting:${setting.id}`,
        kind: "setting",
        title: setting.title,
        detail: `${setting.group} · ${setting.detail}`,
        pageId: "settings",
      },
      `${setting.keywords} setting settings advanced 高级 设置`,
    );
  }

  return matches
    .sort((left, right) => left.rank - right.rank || left.index - right.index)
    .slice(0, 12)
    .map(({ result }) => result);
}

function searchMatchRank(haystack: string, query: string): number | null {
  if (haystack === query) return 0;
  if (haystack.startsWith(query)) return 1;
  if (haystack.split(/[^\p{L}\p{N}]+/u).some((word) => word.startsWith(query))) {
    return 2;
  }
  return haystack.includes(query) ? 3 : null;
}
