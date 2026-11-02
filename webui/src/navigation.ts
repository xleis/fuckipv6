import navigationContract from "../../crates/common/assets/ui-navigation.json";

export type NavigationModuleContract = {
  id: string;
  labelKey: string;
  defaultPageId: string;
  pageIds: string[];
};

type RawNavigationContract = {
  schema_version: number;
  modules: Array<{
    id: string;
    label_key: string;
    default_page_id: string;
    page_ids: string[];
  }>;
};

/**
 * Validate the shared Rust/React navigation contract at module load. A broken
 * contract must fail closed instead of silently rendering an incomplete menu.
 */
function parseNavigationContract(contract: RawNavigationContract): NavigationModuleContract[] {
  if (contract.schema_version !== 1) {
    throw new Error(`Unsupported navigation contract schema: ${contract.schema_version}`);
  }

  const moduleIds = new Set<string>();
  const pageIds = new Set<string>();
  const modules = contract.modules.map((module) => {
    if (!module.id || !module.label_key || module.page_ids.length === 0) {
      throw new Error("Navigation module is missing required fields");
    }
    if (moduleIds.has(module.id)) {
      throw new Error(`Duplicate navigation module: ${module.id}`);
    }
    if (!module.page_ids.includes(module.default_page_id)) {
      throw new Error(`Navigation module ${module.id} has an invalid default page`);
    }
    moduleIds.add(module.id);
    for (const pageId of module.page_ids) {
      if (pageIds.has(pageId)) {
        throw new Error(`Page ${pageId} belongs to more than one navigation module`);
      }
      pageIds.add(pageId);
    }
    return {
      id: module.id,
      labelKey: module.label_key,
      defaultPageId: module.default_page_id,
      pageIds: [...module.page_ids],
    };
  });

  if (modules.length !== 5 || pageIds.size !== 12) {
    throw new Error("The navigation contract must contain five modules and twelve pages");
  }
  return modules;
}

export const NAVIGATION_MODULES = parseNavigationContract(navigationContract);

export function navigationModuleForPage(pageId: string): NavigationModuleContract {
  return (
    NAVIGATION_MODULES.find((module) => module.pageIds.includes(pageId)) ?? NAVIGATION_MODULES[0]
  );
}
