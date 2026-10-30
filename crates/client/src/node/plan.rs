use anyhow::{Context, Result};
use fuckipv6_common::ClientConfig;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::config_manager::ConfigManager;

/// How a node should choose the networks it loads at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeNetworkSelection {
    /// Keep the node and its control surfaces running without joining a network.
    None,
    Default,
    All,
    Named(Vec<String>),
}

/// Fully loaded network configuration ready for a network runtime.
#[derive(Debug, Clone)]
pub struct NodeNetworkSpec {
    pub name: String,
    pub config_path: PathBuf,
    pub config: ClientConfig,
}

/// Validated startup plan shared by every node entry point.
#[derive(Debug, Clone, Default)]
pub struct NodeRuntimePlan {
    networks: Vec<NodeNetworkSpec>,
}

impl NodeRuntimePlan {
    pub fn load(manager: &ConfigManager, selection: NodeNetworkSelection) -> Result<Self> {
        let names = match selection {
            NodeNetworkSelection::None => Vec::new(),
            NodeNetworkSelection::Default => vec![manager
                .get_default_network()?
                .context("未设置默认网络。请指定网络名称或使用 --all")?],
            NodeNetworkSelection::All => manager
                .list_networks()?
                .into_iter()
                .map(|network| network.name)
                .collect(),
            NodeNetworkSelection::Named(names) => {
                if names.is_empty() {
                    anyhow::bail!("至少需要指定一个网络名称");
                }
                names
            }
        };

        let mut seen = HashSet::new();
        let mut networks = Vec::with_capacity(names.len());

        for name in names {
            if !seen.insert(name.clone()) {
                anyhow::bail!("网络重复: {name}");
            }

            let config_path = manager
                .get_network_config(&name)
                .with_context(|| format!("无法加载网络 [{name}] 的配置路径"))?;
            let config = ClientConfig::from_file(&config_path)
                .with_context(|| format!("加载网络 [{name}] 配置文件失败"))?;
            networks.push(NodeNetworkSpec {
                name,
                config_path,
                config,
            });
        }

        Ok(Self { networks })
    }

    pub fn networks(&self) -> &[NodeNetworkSpec] {
        &self.networks
    }

    pub fn into_networks(self) -> Vec<NodeNetworkSpec> {
        self.networks
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.networks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_config_dir(name: &str) -> PathBuf {
        let unique = format!("fuckipv6-node-plan-{}-{}", name, std::process::id());
        std::env::temp_dir().join(unique)
    }

    fn manager(name: &str) -> (ConfigManager, PathBuf) {
        let dir = temp_config_dir(name);
        let _ = std::fs::remove_dir_all(&dir);
        let manager = ConfigManager::new_for_dir(dir.clone()).expect("create test manager");
        (manager, dir)
    }

    #[test]
    fn empty_plan_keeps_node_available_without_a_network() -> Result<()> {
        let (manager, dir) = manager("empty");
        let plan = NodeRuntimePlan::load(&manager, NodeNetworkSelection::None)?;

        assert!(plan.is_empty());
        assert_eq!(plan.len(), 0);

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn default_plan_loads_one_network() -> Result<()> {
        let (manager, dir) = manager("default");
        manager.create_network("home", "ws://home.example:6666", true)?;

        let plan = NodeRuntimePlan::load(&manager, NodeNetworkSelection::Default)?;
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.networks()[0].name, "home");
        assert_eq!(
            plan.networks()[0].config.coordinator_url,
            "ws://home.example:6666"
        );

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn named_plan_loads_multiple_networks_and_rejects_duplicates() -> Result<()> {
        let (manager, dir) = manager("multiple");
        manager.create_network("home", "ws://home.example:6666", false)?;
        manager.create_network("office", "ws://office.example:6666", false)?;

        let plan = NodeRuntimePlan::load(
            &manager,
            NodeNetworkSelection::Named(vec!["home".into(), "office".into()]),
        )?;
        assert_eq!(plan.len(), 2);
        assert!(plan.networks().iter().any(|network| network.name == "home"));
        assert!(plan
            .networks()
            .iter()
            .any(|network| network.name == "office"));

        let duplicate = NodeRuntimePlan::load(
            &manager,
            NodeNetworkSelection::Named(vec!["home".into(), "home".into()]),
        );
        assert!(duplicate.is_err());

        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }
}
