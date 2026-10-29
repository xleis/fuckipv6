#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    pub destination: String,
    pub via: Option<String>,
    pub dev: Option<String>,
    pub metric: Option<u32>,
    pub raw: String,
}

impl RouteEntry {
    pub fn from_linux_ip_route_line(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let mut via = None;
        let mut dev = None;
        let mut metric = None;

        let mut index = 1;
        while index < parts.len() {
            match parts[index] {
                "via" if index + 1 < parts.len() => {
                    via = Some(parts[index + 1].to_string());
                    index += 2;
                }
                "dev" if index + 1 < parts.len() => {
                    dev = Some(parts[index + 1].to_string());
                    index += 2;
                }
                "metric" if index + 1 < parts.len() => {
                    metric = parts[index + 1].parse().ok();
                    index += 2;
                }
                _ => {
                    index += 1;
                }
            }
        }

        Some(Self {
            destination: parts[0].to_string(),
            via,
            dev,
            metric,
            raw: line.to_string(),
        })
    }

    pub fn uses_interface(&self, interface: &str) -> bool {
        self.dev.as_deref() == Some(interface)
    }
}

pub fn parse_linux_ip_routes(output: &str) -> Vec<RouteEntry> {
    output
        .lines()
        .filter_map(RouteEntry::from_linux_ip_route_line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_linux_route_with_dev_and_metric() {
        let route = RouteEntry::from_linux_ip_route_line(
            "fd00::/64 dev fuckipv6 proto kernel metric 256 pref medium",
        )
        .expect("route line should parse");

        assert_eq!(route.destination, "fd00::/64");
        assert_eq!(route.dev.as_deref(), Some("fuckipv6"));
        assert_eq!(route.metric, Some(256));
        assert!(route.uses_interface("fuckipv6"));
    }

    #[test]
    fn parse_linux_route_with_gateway() {
        let route = RouteEntry::from_linux_ip_route_line(
            "default via fe80::1 dev eth0 proto ra metric 100 pref medium",
        )
        .expect("default route line should parse");

        assert_eq!(route.destination, "default");
        assert_eq!(route.via.as_deref(), Some("fe80::1"));
        assert_eq!(route.dev.as_deref(), Some("eth0"));
        assert!(!route.uses_interface("fuckipv6"));
    }
}
