//! Side-effect-free policy evaluation for the administrator policy editor.
//!
//! This module owns no authorization logic. It converts transport-safe input
//! into the same `AccessContext` and `PermissionRequest` consumed by
//! [`evaluate_effective_access`], then projects the authoritative result with
//! field-level validation and explanatory metadata.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::access_control::{
    evaluate_effective_access, EffectiveAccess, EffectiveAccessReason, EffectivePolicyLayer,
    NetworkAcl, NetworkAclGrant, NodeContributionVeto, NodeLocalAccessPolicy, NodeNetworkScope,
};
use super::ids::{GroupId, NetworkId, NodeId, SecurityRuleId};
use super::permissions::{
    role_baseline_policy, role_ceiling_allows, AccessContext, PermissionCondition,
    PermissionEffect, PermissionIntent, PermissionRequest, PermissionRule, PolicyAction,
    ResourceKind, ResourceRef, SubjectSelector,
};

/// Complete, transport-safe input for a policy test. Candidate documents are
/// evaluated in memory and are never applied to the node.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyTestInput {
    pub network_id: NetworkId,
    pub actor: NodeId,
    pub role: super::permissions::NetworkRole,
    #[serde(default)]
    pub groups: BTreeSet<GroupId>,
    pub membership_active: bool,
    pub recent_reauthentication: bool,
    pub emergency_coordinator: bool,
    pub request: PolicyTestRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_policy: Option<NodeLocalAccessPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_acl: Option<NetworkAcl>,
    pub now_unix_ms: u64,
}

/// Strictly parse the complete advanced-mode policy test document. Unknown
/// fields, duplicate fields, malformed values and trailing input are rejected
/// with a field path that the editor can highlight before sending a test.
pub fn parse_policy_test_input(json: &str) -> Result<PolicyTestInput, PolicyValidationIssue> {
    // `serde_json` uses the last value for duplicate object fields. Policy
    // editing must never silently discard a candidate rule, so detect repeated
    // keys before deserialization and preserve the exact JSON Pointer.
    if let Some(issue) = duplicate_object_key_issue(json) {
        return Err(issue);
    }
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let parsed = serde_path_to_error::deserialize::<_, PolicyTestInput>(&mut deserializer)
        .map_err(|error| {
            json_parse_issue(
                &serde_path_to_json_pointer(&error.path().to_string()),
                error.inner().to_string(),
            )
        })?;
    deserializer
        .end()
        .map_err(|error| json_parse_issue("/", error.to_string()))?;
    Ok(parsed)
}

/// Scan syntactically valid-enough JSON for repeated object keys. The scanner
/// is deliberately conservative: malformed JSON is left to serde_json so the
/// user receives the authoritative syntax error. Its only job is to close the
/// duplicate-key ambiguity that `JSON.parse` and serde normally collapse.
fn duplicate_object_key_issue(json: &str) -> Option<PolicyValidationIssue> {
    struct Scanner<'a> {
        source: &'a str,
        bytes: &'a [u8],
        position: usize,
    }

    impl Scanner<'_> {
        fn skip_whitespace(&mut self) {
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.position += 1;
            }
        }

        fn string_token(&mut self) -> Option<(String, usize, usize)> {
            self.skip_whitespace();
            let start = self.position;
            if self.bytes.get(self.position) != Some(&b'"') {
                return None;
            }
            self.position += 1;
            let mut escaped = false;
            while let Some(byte) = self.bytes.get(self.position).copied() {
                self.position += 1;
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    let token = &self.source[start..self.position];
                    let decoded = serde_json::from_str::<String>(token).ok()?;
                    return Some((decoded, start, self.position));
                }
            }
            None
        }

        fn value(&mut self, path: &str) -> Option<Option<PolicyValidationIssue>> {
            self.skip_whitespace();
            match self.bytes.get(self.position).copied()? {
                b'{' => self.object(path),
                b'[' => self.array(path),
                b'"' => {
                    self.string_token()?;
                    Some(None)
                }
                b't' | b'f' | b'n' | b'-' | b'0'..=b'9' => {
                    while let Some(byte) = self.bytes.get(self.position).copied() {
                        if byte.is_ascii_whitespace() || matches!(byte, b',' | b']' | b'}') {
                            break;
                        }
                        self.position += 1;
                    }
                    Some(None)
                }
                _ => None,
            }
        }

        fn object(&mut self, path: &str) -> Option<Option<PolicyValidationIssue>> {
            let mut keys = BTreeSet::new();
            self.position += 1;
            self.skip_whitespace();
            if self.bytes.get(self.position) == Some(&b'}') {
                self.position += 1;
                return Some(None);
            }
            loop {
                let (key, _, _) = self.string_token()?;
                let pointer = format!("{path}/{}", json_pointer_escape(&key));
                if !keys.insert(key.clone()) {
                    return Some(Some(json_parse_issue(
                        &pointer,
                        format!("duplicate field `{key}` is not allowed"),
                    )));
                }
                self.skip_whitespace();
                if self.bytes.get(self.position) != Some(&b':') {
                    return None;
                }
                self.position += 1;
                if let Some(issue) = self.value(&pointer)? {
                    return Some(Some(issue));
                }
                self.skip_whitespace();
                match self.bytes.get(self.position).copied()? {
                    b',' => self.position += 1,
                    b'}' => {
                        self.position += 1;
                        return Some(None);
                    }
                    _ => return None,
                }
            }
        }

        fn array(&mut self, path: &str) -> Option<Option<PolicyValidationIssue>> {
            self.position += 1;
            self.skip_whitespace();
            if self.bytes.get(self.position) == Some(&b']') {
                self.position += 1;
                return Some(None);
            }
            let mut index = 0usize;
            loop {
                let child = format!("{path}/{index}");
                if let Some(issue) = self.value(&child)? {
                    return Some(Some(issue));
                }
                self.skip_whitespace();
                match self.bytes.get(self.position).copied()? {
                    b',' => {
                        self.position += 1;
                        index = index.saturating_add(1);
                    }
                    b']' => {
                        self.position += 1;
                        return Some(None);
                    }
                    _ => return None,
                }
            }
        }
    }

    let mut scanner = Scanner {
        source: json,
        bytes: json.as_bytes(),
        position: 0,
    };
    scanner.skip_whitespace();
    let issue = scanner.value("")?;
    // A duplicate-key finding stops the scan immediately, so it must be
    // returned before checking whether the entire document was consumed.
    if issue.is_some() {
        return issue;
    }
    scanner.skip_whitespace();
    if scanner.position != scanner.bytes.len() {
        return None;
    }
    None
}

fn json_pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn json_parse_issue(path: &str, message: String) -> PolicyValidationIssue {
    PolicyValidationIssue {
        path: path.into(),
        code: "invalid_test_input_json".into(),
        source: PolicyValidationSource::TestInput,
        related_rule_id: None,
        message,
    }
}

fn serde_path_to_json_pointer(path: &str) -> String {
    if path.is_empty() || path == "." {
        return "/".into();
    }
    let mut pointer = String::new();
    let mut chars = path.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '.' => pointer.push('/'),
            '[' => {
                pointer.push('/');
                for digit in chars.by_ref() {
                    if digit == ']' {
                        break;
                    }
                    pointer.push(digit);
                }
            }
            _ => pointer.push(character),
        }
    }
    if pointer.is_empty() {
        "/".into()
    } else if pointer.starts_with('/') {
        pointer
    } else {
        format!("/{pointer}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyTestRequest {
    pub action: PolicyAction,
    pub resource: ResourceKind,
    pub resource_network_id: NetworkId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    pub intent: PermissionIntent,
}

/// Stable source labels. They identify where a rule or factor came from
/// without changing the evaluator's final decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRuleSource {
    RoleBaseline,
    LocalPolicyRule,
    LocalContributionVeto,
    LocalNetworkScope,
    NetworkAclRule,
    NetworkAclGrant,
    Request,
    AccessContext,
}

impl PolicyRuleSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoleBaseline => "role_baseline",
            Self::LocalPolicyRule => "local_policy_rule",
            Self::LocalContributionVeto => "local_contribution_veto",
            Self::LocalNetworkScope => "local_network_scope",
            Self::NetworkAclRule => "network_acl_rule",
            Self::NetworkAclGrant => "network_acl_grant",
            Self::Request => "request",
            Self::AccessContext => "access_context",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyValidationSource {
    LocalPolicy,
    NetworkAcl,
    TestInput,
}

impl PolicyValidationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalPolicy => "local_policy",
            Self::NetworkAcl => "network_acl",
            Self::TestInput => "test_input",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyValidationIssue {
    /// JSON-pointer-style path into the submitted policy document.
    pub path: String,
    pub code: String,
    pub source: PolicyValidationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_rule_id: Option<SecurityRuleId>,
    pub message: String,
}

/// Fixed precedence order consumed by the UI. The evaluator remains the sole
/// authority for the selected outcome; this trace only explains which gate was
/// active and in what order gates were considered.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PolicyPrecedence {
    CrossNetworkDenied,
    InvalidOrExpiredPolicy,
    NetworkScopeRestricted,
    LocalContributionVeto,
    ExplicitDeny,
    TemporaryCoordinatorReadOnly,
    RoleCeilingExceeded,
    AuthorizationRevoked,
    AuthorizationExpired,
    RecentReauthenticationRequired,
    DefaultDenied,
    Allow,
}

impl PolicyPrecedence {
    pub const ALL: [Self; 12] = [
        Self::CrossNetworkDenied,
        Self::InvalidOrExpiredPolicy,
        Self::NetworkScopeRestricted,
        Self::LocalContributionVeto,
        Self::ExplicitDeny,
        Self::TemporaryCoordinatorReadOnly,
        Self::RoleCeilingExceeded,
        Self::AuthorizationRevoked,
        Self::AuthorizationExpired,
        Self::RecentReauthenticationRequired,
        Self::DefaultDenied,
        Self::Allow,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CrossNetworkDenied => "cross_network_denied",
            Self::InvalidOrExpiredPolicy => "invalid_or_expired_policy",
            Self::NetworkScopeRestricted => "network_scope_restricted",
            Self::LocalContributionVeto => "local_contribution_veto",
            Self::ExplicitDeny => "explicit_deny",
            Self::TemporaryCoordinatorReadOnly => "temporary_coordinator_read_only",
            Self::RoleCeilingExceeded => "role_ceiling_exceeded",
            Self::AuthorizationRevoked => "authorization_revoked",
            Self::AuthorizationExpired => "authorization_expired",
            Self::RecentReauthenticationRequired => "recent_reauthentication_required",
            Self::DefaultDenied => "default_denied",
            Self::Allow => "allow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyRuleExplanation {
    pub source: PolicyRuleSource,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<SecurityRuleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<PermissionEffect>,
    #[serde(default)]
    pub subjects: Vec<SubjectSelector>,
    #[serde(default)]
    pub resources: Vec<ResourceKind>,
    #[serde(default)]
    pub actions: Vec<PolicyAction>,
    #[serde(default)]
    pub conditions: Vec<PermissionCondition>,
    /// Whether this rule/factor matched the submitted request context.
    pub matched: bool,
    /// Whether this rule/factor could contribute to the evaluated decision.
    /// Revoked, expired and stale-reauthentication grants are matched but not
    /// applied.
    pub applied: bool,
    /// The precedence label associated with this rule or factor.
    pub precedence: PolicyPrecedence,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyPrecedenceExplanation {
    pub precedence: PolicyPrecedence,
    pub triggered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PolicyRuleSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<SecurityRuleId>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyTestResult {
    /// Final outcome and all matched identifiers from the authoritative runtime
    /// evaluator. Consumers must display these fields as-is.
    pub decision: EffectiveAccess,
    pub validation_issues: Vec<PolicyValidationIssue>,
    pub explanations: Vec<PolicyRuleExplanation>,
    pub precedence: Vec<PolicyPrecedenceExplanation>,
}

impl PolicyTestInput {
    /// Evaluate the candidate documents with the exact runtime evaluator. This
    /// function borrows all input and has no storage, audit, activity or state
    /// handle, so it cannot execute an authorization side effect.
    pub fn evaluate(&self) -> PolicyTestResult {
        let validation_issues = validate_policy_test_input(self);
        let baseline = role_baseline_policy(&self.network_id, self.role);
        let request = self.permission_request();
        let context = self.access_context();
        let decision = evaluate_effective_access(
            &baseline,
            &baseline,
            self.network_acl.as_ref(),
            self.local_policy.as_ref(),
            &request,
            &context,
            self.now_unix_ms,
        );
        let explanations = self.explanations(&baseline, &request, &context, &decision);
        let precedence = precedence_trace(&decision);
        PolicyTestResult {
            decision,
            validation_issues,
            explanations,
            precedence,
        }
    }

    fn permission_request(&self) -> PermissionRequest {
        PermissionRequest {
            action: self.request.action,
            resource: ResourceRef {
                kind: self.request.resource,
                network_id: self.request.resource_network_id.clone(),
                object_id: self.request.object_id.clone(),
            },
            intent: self.request.intent,
        }
    }

    fn access_context(&self) -> AccessContext {
        AccessContext {
            network_id: self.network_id.clone(),
            actor: self.actor.clone(),
            role: self.role,
            groups: self.groups.clone(),
            membership_active: self.membership_active,
            recent_reauthentication: self.recent_reauthentication,
            emergency_coordinator: self.emergency_coordinator,
        }
    }

    fn explanations(
        &self,
        baseline: &[PermissionRule],
        request: &PermissionRequest,
        context: &AccessContext,
        decision: &EffectiveAccess,
    ) -> Vec<PolicyRuleExplanation> {
        let mut explanations = Vec::new();
        append_rule_explanations(
            &mut explanations,
            RuleExplanationSource {
                source: PolicyRuleSource::RoleBaseline,
                base_path: "/role_baseline/rules",
                rules: baseline,
                layer: EffectivePolicyLayer::Network,
            },
            request,
            context,
            decision,
        );
        if let Some(policy) = &self.local_policy {
            let scope_matched = !policy.network_scope.allows(&context.network_id);
            explanations.push(PolicyRuleExplanation {
                source: PolicyRuleSource::LocalNetworkScope,
                source_path: "/local_policy/network_scope".into(),
                rule_id: None,
                effect: Some(PermissionEffect::Deny),
                subjects: Vec::new(),
                resources: vec![ResourceKind::Network],
                actions: Vec::new(),
                conditions: Vec::new(),
                matched: scope_matched,
                applied: scope_matched
                    && decision.reason == EffectiveAccessReason::NetworkScopeRestricted,
                precedence: PolicyPrecedence::NetworkScopeRestricted,
            });
            append_rule_explanations(
                &mut explanations,
                RuleExplanationSource {
                    source: PolicyRuleSource::LocalPolicyRule,
                    base_path: "/local_policy/rules",
                    rules: &policy.rules,
                    layer: EffectivePolicyLayer::Local,
                },
                request,
                context,
                decision,
            );
            for (index, veto) in policy.contribution_vetoes.iter().enumerate() {
                let contribution = veto.matches(request, &context.network_id);
                let matched = contribution.is_some();
                explanations.push(PolicyRuleExplanation {
                    source: PolicyRuleSource::LocalContributionVeto,
                    source_path: format!("/local_policy/contribution_vetoes/{index}"),
                    rule_id: Some(veto.veto_id.clone()),
                    effect: Some(PermissionEffect::Deny),
                    subjects: Vec::new(),
                    resources: contribution
                        .map(|kind| vec![kind.resource_kind()])
                        .unwrap_or_default(),
                    actions: vec![PolicyAction::Use],
                    conditions: Vec::new(),
                    matched,
                    applied: matched
                        && decision.reason == EffectiveAccessReason::LocalContributionVeto
                        && decision.rule_id.as_ref() == Some(&veto.veto_id),
                    precedence: PolicyPrecedence::LocalContributionVeto,
                });
            }
        }
        if let Some(acl) = &self.network_acl {
            append_rule_explanations(
                &mut explanations,
                RuleExplanationSource {
                    source: PolicyRuleSource::NetworkAclRule,
                    base_path: "/network_acl/rules",
                    rules: &acl.rules,
                    layer: EffectivePolicyLayer::Network,
                },
                request,
                context,
                decision,
            );
            for (index, grant) in acl.grants.iter().enumerate() {
                explanations.push(grant_explanation(
                    grant,
                    index,
                    request,
                    context,
                    self.now_unix_ms,
                    decision,
                ));
            }
        }
        explanations.push(PolicyRuleExplanation {
            source: PolicyRuleSource::Request,
            source_path: "/request".into(),
            rule_id: None,
            effect: None,
            subjects: Vec::new(),
            resources: vec![request.resource.kind],
            actions: vec![request.action],
            conditions: Vec::new(),
            matched: request.resource.network_id != context.network_id,
            applied: decision.reason == EffectiveAccessReason::CrossNetworkDenied,
            precedence: PolicyPrecedence::CrossNetworkDenied,
        });
        explanations.push(PolicyRuleExplanation {
            source: PolicyRuleSource::AccessContext,
            source_path: "/access_context".into(),
            rule_id: None,
            effect: None,
            subjects: vec![SubjectSelector::Role(context.role)],
            resources: Vec::new(),
            actions: Vec::new(),
            conditions: Vec::new(),
            matched: context.role == super::permissions::NetworkRole::TemporaryCoordinator
                && request.intent.is_mutating(),
            applied: decision.reason == EffectiveAccessReason::TemporaryCoordinatorReadOnly,
            precedence: PolicyPrecedence::TemporaryCoordinatorReadOnly,
        });
        explanations
    }
}

struct RuleExplanationSource<'a> {
    source: PolicyRuleSource,
    base_path: &'a str,
    rules: &'a [PermissionRule],
    layer: EffectivePolicyLayer,
}

fn append_rule_explanations(
    explanations: &mut Vec<PolicyRuleExplanation>,
    source: RuleExplanationSource<'_>,
    request: &PermissionRequest,
    context: &AccessContext,
    decision: &EffectiveAccess,
) {
    for (index, rule) in source.rules.iter().enumerate() {
        let matched = rule.matches(request, context);
        let precedence = match rule.effect {
            PermissionEffect::Deny => PolicyPrecedence::ExplicitDeny,
            PermissionEffect::Allow
                if matched
                    && !role_ceiling_allows(
                        context.role,
                        request.action,
                        request.resource.kind,
                    ) =>
            {
                PolicyPrecedence::RoleCeilingExceeded
            }
            PermissionEffect::Allow => PolicyPrecedence::Allow,
        };
        let applied = matched
            && match rule.effect {
                PermissionEffect::Deny => {
                    decision.reason == EffectiveAccessReason::ExplicitDeny
                        && decision.layer == Some(source.layer)
                        && decision.rule_id.as_ref() == Some(&rule.rule_id)
                }
                PermissionEffect::Allow => decision.allowed,
            };
        explanations.push(PolicyRuleExplanation {
            source: source.source,
            source_path: format!("{}/{index}", source.base_path),
            rule_id: Some(rule.rule_id.clone()),
            effect: Some(rule.effect),
            subjects: rule.subjects.clone(),
            resources: rule.resources.clone(),
            actions: rule.actions.clone(),
            conditions: rule.conditions.clone(),
            matched,
            applied,
            precedence,
        });
    }
}

fn grant_explanation(
    grant: &NetworkAclGrant,
    index: usize,
    request: &PermissionRequest,
    context: &AccessContext,
    now_unix_ms: u64,
    decision: &EffectiveAccess,
) -> PolicyRuleExplanation {
    let matched = grant.matches(request, context);
    let revoked = matched && grant.is_revoked();
    let expired = matched && !revoked && grant.is_expired(now_unix_ms);
    let reauthentication_required = matched
        && !revoked
        && !expired
        && grant.requires_recent_reauthentication
        && !context.recent_reauthentication;
    let precedence = if revoked {
        PolicyPrecedence::AuthorizationRevoked
    } else if expired {
        PolicyPrecedence::AuthorizationExpired
    } else if reauthentication_required {
        PolicyPrecedence::RecentReauthenticationRequired
    } else {
        PolicyPrecedence::Allow
    };
    let mut conditions = vec![PermissionCondition::MembershipActive];
    if grant.requires_recent_reauthentication {
        conditions.push(PermissionCondition::RecentReauthentication);
    }
    PolicyRuleExplanation {
        source: PolicyRuleSource::NetworkAclGrant,
        source_path: format!("/network_acl/grants/{index}"),
        rule_id: Some(grant.grant_id.clone()),
        effect: Some(PermissionEffect::Allow),
        subjects: grant.subjects.clone(),
        resources: grant.resources.clone(),
        actions: grant.actions.clone(),
        conditions,
        matched,
        applied: matched && !revoked && !expired && !reauthentication_required && decision.allowed,
        precedence,
    }
}

fn precedence_trace(decision: &EffectiveAccess) -> Vec<PolicyPrecedenceExplanation> {
    let selected = match decision.reason {
        EffectiveAccessReason::CrossNetworkDenied => PolicyPrecedence::CrossNetworkDenied,
        EffectiveAccessReason::InvalidPolicy => PolicyPrecedence::InvalidOrExpiredPolicy,
        EffectiveAccessReason::NetworkScopeRestricted => PolicyPrecedence::NetworkScopeRestricted,
        EffectiveAccessReason::LocalContributionVeto => PolicyPrecedence::LocalContributionVeto,
        EffectiveAccessReason::ExplicitDeny => PolicyPrecedence::ExplicitDeny,
        EffectiveAccessReason::TemporaryCoordinatorReadOnly => {
            PolicyPrecedence::TemporaryCoordinatorReadOnly
        }
        EffectiveAccessReason::RoleCeilingExceeded => PolicyPrecedence::RoleCeilingExceeded,
        EffectiveAccessReason::AuthorizationRevoked => PolicyPrecedence::AuthorizationRevoked,
        EffectiveAccessReason::AuthorizationExpired => PolicyPrecedence::AuthorizationExpired,
        EffectiveAccessReason::RecentReauthenticationRequired => {
            PolicyPrecedence::RecentReauthenticationRequired
        }
        EffectiveAccessReason::DefaultDenied => PolicyPrecedence::DefaultDenied,
        EffectiveAccessReason::Allowed => PolicyPrecedence::Allow,
    };
    let source = match selected {
        PolicyPrecedence::CrossNetworkDenied => Some(PolicyRuleSource::Request),
        PolicyPrecedence::TemporaryCoordinatorReadOnly => Some(PolicyRuleSource::AccessContext),
        PolicyPrecedence::NetworkScopeRestricted => Some(PolicyRuleSource::LocalNetworkScope),
        PolicyPrecedence::LocalContributionVeto => Some(PolicyRuleSource::LocalContributionVeto),
        PolicyPrecedence::InvalidOrExpiredPolicy
        | PolicyPrecedence::ExplicitDeny
        | PolicyPrecedence::RoleCeilingExceeded => Some(match decision.layer {
            Some(EffectivePolicyLayer::Local) => PolicyRuleSource::LocalPolicyRule,
            Some(EffectivePolicyLayer::Network) => PolicyRuleSource::NetworkAclRule,
            Some(EffectivePolicyLayer::Request) | None => PolicyRuleSource::Request,
        }),
        PolicyPrecedence::AuthorizationRevoked
        | PolicyPrecedence::AuthorizationExpired
        | PolicyPrecedence::RecentReauthenticationRequired => Some(if decision.rule_id.is_some() {
            PolicyRuleSource::NetworkAclGrant
        } else {
            match decision.layer {
                Some(EffectivePolicyLayer::Local) => PolicyRuleSource::LocalPolicyRule,
                Some(EffectivePolicyLayer::Network) => PolicyRuleSource::NetworkAclRule,
                Some(EffectivePolicyLayer::Request) | None => PolicyRuleSource::Request,
            }
        }),
        PolicyPrecedence::DefaultDenied | PolicyPrecedence::Allow => None,
    };
    PolicyPrecedence::ALL
        .into_iter()
        .map(|precedence| PolicyPrecedenceExplanation {
            precedence,
            triggered: precedence == selected,
            source: (precedence == selected).then_some(source).flatten(),
            rule_id: (precedence == selected)
                .then(|| decision.rule_id.clone())
                .flatten(),
        })
        .collect()
}

/// Validate candidate documents without mutating them or invoking storage.
pub fn validate_policy_test_input(input: &PolicyTestInput) -> Vec<PolicyValidationIssue> {
    let mut issues = Vec::new();
    if (input.role == super::permissions::NetworkRole::TemporaryCoordinator)
        != input.emergency_coordinator
    {
        push_issue(
            &mut issues,
            "/emergency_coordinator",
            "temporary_coordinator_context_mismatch",
            PolicyValidationSource::TestInput,
            None,
            "emergency_coordinator must match the TemporaryCoordinator role",
        );
    }

    if let Some(policy) = &input.local_policy {
        validate_local_policy(input, policy, &mut issues);
    }
    if let Some(acl) = &input.network_acl {
        validate_network_acl(input, acl, &mut issues);
    }
    issues
}

fn validate_local_policy(
    input: &PolicyTestInput,
    policy: &NodeLocalAccessPolicy,
    issues: &mut Vec<PolicyValidationIssue>,
) {
    if policy.node_id != input.actor {
        push_issue(
            issues,
            "/local_policy/node_id",
            "local_policy_node_mismatch",
            PolicyValidationSource::LocalPolicy,
            None,
            "local policy node_id must match the tested actor",
        );
    }
    validate_revision(
        issues,
        "/local_policy/revision",
        policy.revision,
        PolicyValidationSource::LocalPolicy,
    );
    validate_validity_interval(
        issues,
        "/local_policy/issued_at_unix_ms",
        "/local_policy/expires_at_unix_ms",
        policy.issued_at_unix_ms,
        policy.expires_at_unix_ms,
        "local_policy_validity_interval_empty",
        PolicyValidationSource::LocalPolicy,
    );
    validate_network_scope(&policy.network_scope, issues);

    let mut rule_paths = BTreeMap::new();
    for (index, rule) in policy.rules.iter().enumerate() {
        let path = format!("/local_policy/rules/{index}");
        validate_rule(
            &path,
            rule,
            None,
            PolicyValidationSource::LocalPolicy,
            issues,
        );
        validate_unique_id(
            issues,
            &mut rule_paths,
            &rule.rule_id,
            &format!("{path}/rule_id"),
            "duplicate_local_policy_rule_id",
            PolicyValidationSource::LocalPolicy,
            "local policy rule id must be unique",
        );
    }
    let mut veto_paths = BTreeMap::new();
    for (index, veto) in policy.contribution_vetoes.iter().enumerate() {
        let path = format!("/local_policy/contribution_vetoes/{index}");
        validate_veto(&path, veto, issues);
        validate_unique_id(
            issues,
            &mut veto_paths,
            &veto.veto_id,
            &format!("{path}/veto_id"),
            "duplicate_local_veto_id",
            PolicyValidationSource::LocalPolicy,
            "local contribution veto id must be unique",
        );
        if let Some(rule_path) = rule_paths.get(&veto.veto_id) {
            push_issue(
                issues,
                &format!("{path}/veto_id"),
                "rule_veto_id_conflict",
                PolicyValidationSource::LocalPolicy,
                Some(veto.veto_id.clone()),
                format!("veto id conflicts with local rule at {rule_path}"),
            );
        }
    }
}

fn validate_network_acl(
    input: &PolicyTestInput,
    acl: &NetworkAcl,
    issues: &mut Vec<PolicyValidationIssue>,
) {
    if acl.network_id != input.network_id {
        push_issue(
            issues,
            "/network_acl/network_id",
            "network_acl_network_mismatch",
            PolicyValidationSource::NetworkAcl,
            None,
            "network ACL network_id must match the tested network",
        );
    }
    validate_revision(
        issues,
        "/network_acl/revision",
        acl.revision,
        PolicyValidationSource::NetworkAcl,
    );
    validate_validity_interval(
        issues,
        "/network_acl/issued_at_unix_ms",
        "/network_acl/expires_at_unix_ms",
        acl.issued_at_unix_ms,
        acl.expires_at_unix_ms,
        "network_acl_validity_interval_empty",
        PolicyValidationSource::NetworkAcl,
    );

    let mut rule_paths = BTreeMap::new();
    for (index, rule) in acl.rules.iter().enumerate() {
        let path = format!("/network_acl/rules/{index}");
        validate_rule(
            &path,
            rule,
            Some(&input.network_id),
            PolicyValidationSource::NetworkAcl,
            issues,
        );
        validate_unique_id(
            issues,
            &mut rule_paths,
            &rule.rule_id,
            &format!("{path}/rule_id"),
            "duplicate_network_acl_rule_id",
            PolicyValidationSource::NetworkAcl,
            "network ACL rule id must be unique",
        );
    }
    let mut grant_paths = BTreeMap::new();
    for (index, grant) in acl.grants.iter().enumerate() {
        let path = format!("/network_acl/grants/{index}");
        validate_grant(&path, acl, grant, issues);
        validate_unique_id(
            issues,
            &mut grant_paths,
            &grant.grant_id,
            &format!("{path}/grant_id"),
            "duplicate_network_acl_grant_id",
            PolicyValidationSource::NetworkAcl,
            "network ACL grant id must be unique",
        );
        if let Some(rule_path) = rule_paths.get(&grant.grant_id) {
            push_issue(
                issues,
                &format!("{path}/grant_id"),
                "rule_grant_id_conflict",
                PolicyValidationSource::NetworkAcl,
                Some(grant.grant_id.clone()),
                format!("grant id conflicts with network ACL rule at {rule_path}"),
            );
        }
    }
}

fn validate_network_scope(scope: &NodeNetworkScope, issues: &mut Vec<PolicyValidationIssue>) {
    let NodeNetworkScope::Only(network_ids) = scope else {
        return;
    };
    if network_ids.is_empty() {
        push_issue(
            issues,
            "/local_policy/network_scope/network_ids",
            "empty_network_scope",
            PolicyValidationSource::LocalPolicy,
            None,
            "network scope allow-list cannot be empty",
        );
    }
    let mut seen = BTreeMap::new();
    for (index, network_id) in network_ids.iter().enumerate() {
        if let Some(first) = seen.insert(network_id, index) {
            push_issue(
                issues,
                &format!("/local_policy/network_scope/network_ids/{index}"),
                "duplicate_network_scope_id",
                PolicyValidationSource::LocalPolicy,
                None,
                format!("network id duplicates entry {first}"),
            );
        }
    }
}

fn validate_rule(
    path: &str,
    rule: &PermissionRule,
    required_network: Option<&NetworkId>,
    source: PolicyValidationSource,
    issues: &mut Vec<PolicyValidationIssue>,
) {
    validate_non_empty(
        path,
        "subjects",
        rule.subjects.len(),
        "empty_rule_subjects",
        source,
        issues,
    );
    validate_non_empty(
        path,
        "resources",
        rule.resources.len(),
        "empty_rule_resources",
        source,
        issues,
    );
    validate_non_empty(
        path,
        "actions",
        rule.actions.len(),
        "empty_rule_actions",
        source,
        issues,
    );
    validate_unique_values(
        issues,
        &format!("{path}/subjects"),
        &rule.subjects,
        "duplicate_rule_subject",
        source,
    );
    validate_unique_values(
        issues,
        &format!("{path}/resources"),
        &rule.resources,
        "duplicate_rule_resource",
        source,
    );
    validate_unique_values(
        issues,
        &format!("{path}/actions"),
        &rule.actions,
        "duplicate_rule_action",
        source,
    );
    validate_unique_values(
        issues,
        &format!("{path}/conditions"),
        &rule.conditions,
        "duplicate_rule_condition",
        source,
    );
    if let Some(network_id) = required_network {
        for (index, condition) in rule.conditions.iter().enumerate() {
            if let PermissionCondition::ResourceInNetwork(condition_network) = condition {
                if condition_network != network_id {
                    push_issue(
                        issues,
                        &format!("{path}/conditions/{index}"),
                        "cross_network_rule_condition",
                        source,
                        Some(rule.rule_id.clone()),
                        format!(
                            "rule condition references network {condition_network} instead of {network_id}"
                        ),
                    );
                }
            }
        }
    }
}

fn validate_grant(
    path: &str,
    acl: &NetworkAcl,
    grant: &NetworkAclGrant,
    issues: &mut Vec<PolicyValidationIssue>,
) {
    validate_non_empty(
        path,
        "subjects",
        grant.subjects.len(),
        "empty_grant_subjects",
        PolicyValidationSource::NetworkAcl,
        issues,
    );
    validate_non_empty(
        path,
        "resources",
        grant.resources.len(),
        "empty_grant_resources",
        PolicyValidationSource::NetworkAcl,
        issues,
    );
    validate_non_empty(
        path,
        "actions",
        grant.actions.len(),
        "empty_grant_actions",
        PolicyValidationSource::NetworkAcl,
        issues,
    );
    validate_unique_values(
        issues,
        &format!("{path}/subjects"),
        &grant.subjects,
        "duplicate_grant_subject",
        PolicyValidationSource::NetworkAcl,
    );
    validate_unique_values(
        issues,
        &format!("{path}/resources"),
        &grant.resources,
        "duplicate_grant_resource",
        PolicyValidationSource::NetworkAcl,
    );
    validate_unique_values(
        issues,
        &format!("{path}/actions"),
        &grant.actions,
        "duplicate_grant_action",
        PolicyValidationSource::NetworkAcl,
    );
    validate_validity_interval(
        issues,
        &format!("{path}/issued_at_unix_ms"),
        &format!("{path}/expires_at_unix_ms"),
        grant.issued_at_unix_ms,
        grant.expires_at_unix_ms,
        "grant_validity_interval_empty",
        PolicyValidationSource::NetworkAcl,
    );
    if grant.issued_at_unix_ms < acl.issued_at_unix_ms {
        push_issue(
            issues,
            &format!("{path}/issued_at_unix_ms"),
            "grant_predates_acl",
            PolicyValidationSource::NetworkAcl,
            Some(grant.grant_id.clone()),
            "grant cannot predate its containing network ACL revision",
        );
    }
    match (grant.revoked_at_unix_ms, grant.revoked_by.as_ref()) {
        (Some(revoked_at), Some(_)) if revoked_at >= grant.issued_at_unix_ms => {}
        (None, None) => {}
        (Some(revoked_at), _) => {
            push_issue(
                issues,
                &format!("{path}/revoked_at_unix_ms"),
                "incomplete_grant_revocation",
                PolicyValidationSource::NetworkAcl,
                Some(grant.grant_id.clone()),
                if revoked_at < grant.issued_at_unix_ms {
                    "grant revocation cannot predate issuance"
                } else {
                    "grant revocation requires revoked_by"
                },
            );
        }
        (None, Some(_)) => push_issue(
            issues,
            &format!("{path}/revoked_by"),
            "incomplete_grant_revocation",
            PolicyValidationSource::NetworkAcl,
            Some(grant.grant_id.clone()),
            "grant revoked_by requires revoked_at_unix_ms",
        ),
    }
}

fn validate_veto(path: &str, veto: &NodeContributionVeto, issues: &mut Vec<PolicyValidationIssue>) {
    if veto.contributions.is_empty() {
        push_issue(
            issues,
            &format!("{path}/contributions"),
            "empty_veto_contributions",
            PolicyValidationSource::LocalPolicy,
            Some(veto.veto_id.clone()),
            "local contribution veto must contain at least one contribution",
        );
    }
    validate_unique_values(
        issues,
        &format!("{path}/contributions"),
        &veto.contributions,
        "duplicate_veto_contribution",
        PolicyValidationSource::LocalPolicy,
    );
}

fn validate_revision(
    issues: &mut Vec<PolicyValidationIssue>,
    path: &str,
    revision: u64,
    source: PolicyValidationSource,
) {
    if revision == 0 {
        push_issue(
            issues,
            path,
            "invalid_policy_revision",
            source,
            None,
            "policy revision must be positive",
        );
    }
}

fn validate_validity_interval(
    issues: &mut Vec<PolicyValidationIssue>,
    issued_path: &str,
    expires_path: &str,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    code: &str,
    source: PolicyValidationSource,
) {
    if issued_at_unix_ms >= expires_at_unix_ms {
        push_issue(
            issues,
            expires_path,
            code,
            source,
            None,
            format!("validity interval must end after {issued_path}"),
        );
    }
}

fn validate_non_empty(
    path: &str,
    field: &str,
    len: usize,
    code: &str,
    source: PolicyValidationSource,
    issues: &mut Vec<PolicyValidationIssue>,
) {
    if len == 0 {
        push_issue(
            issues,
            &format!("{path}/{field}"),
            code,
            source,
            None,
            format!("{field} cannot be empty"),
        );
    }
}

fn validate_unique_values<T: Ord>(
    issues: &mut Vec<PolicyValidationIssue>,
    path: &str,
    values: &[T],
    code: &str,
    source: PolicyValidationSource,
) {
    let mut seen = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        if let Some(first) = seen.insert(value, index) {
            push_issue(
                issues,
                &format!("{path}/{index}"),
                code,
                source,
                None,
                format!("value duplicates entry {first}"),
            );
        }
    }
}

fn validate_unique_id(
    issues: &mut Vec<PolicyValidationIssue>,
    seen: &mut BTreeMap<SecurityRuleId, usize>,
    id: &SecurityRuleId,
    path: &str,
    code: &str,
    source: PolicyValidationSource,
    message: &str,
) {
    if let Some(first) = seen.insert(id.clone(), seen.len()) {
        push_issue(
            issues,
            path,
            code,
            source,
            Some(id.clone()),
            format!("{message}; first declared at index {first}"),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_issue(
    issues: &mut Vec<PolicyValidationIssue>,
    path: &str,
    code: &str,
    source: PolicyValidationSource,
    related_rule_id: Option<SecurityRuleId>,
    message: impl Into<String>,
) {
    issues.push(PolicyValidationIssue {
        path: path.into(),
        code: code.into(),
        source,
        related_rule_id,
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ContributionKind, NetworkRole, NodeNetworkScope, PermissionCondition, PermissionEffect,
    };

    fn network_id(seed: &[u8]) -> NetworkId {
        NetworkId::from_public_key(seed)
    }

    fn actor() -> NodeId {
        NodeId::from_public_key(b"policy-test-actor")
    }

    fn base_input(role: NetworkRole) -> PolicyTestInput {
        let network_id = network_id(b"policy-test-network");
        PolicyTestInput {
            network_id: network_id.clone(),
            actor: actor(),
            role,
            groups: BTreeSet::new(),
            membership_active: true,
            recent_reauthentication: true,
            emergency_coordinator: role == NetworkRole::TemporaryCoordinator,
            request: PolicyTestRequest {
                action: PolicyAction::Read,
                resource: ResourceKind::Network,
                resource_network_id: network_id,
                object_id: None,
                intent: PermissionIntent::ReadOnly,
            },
            local_policy: None,
            network_acl: None,
            now_unix_ms: 1_000,
        }
    }

    fn rule(id: &[u8], effect: PermissionEffect) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(id),
            effect,
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![ResourceKind::Network],
            actions: vec![PolicyAction::Read],
            conditions: vec![PermissionCondition::MembershipActive],
        }
    }

    fn acl(input: &PolicyTestInput, rules: Vec<PermissionRule>) -> NetworkAcl {
        NetworkAcl {
            network_id: input.network_id.clone(),
            revision: 3,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            rules,
            grants: Vec::new(),
            relay_limits: Default::default(),
        }
    }

    #[test]
    fn role_baseline_is_shared_and_scoped_to_network() {
        let network_id = network_id(b"role-baseline-network");
        let policy = role_baseline_policy(&network_id, NetworkRole::Member);
        assert_eq!(policy.len(), 1);
        assert_eq!(
            policy[0].subjects,
            vec![SubjectSelector::Role(NetworkRole::Member)]
        );
        assert!(policy[0]
            .conditions
            .contains(&PermissionCondition::ResourceInNetwork(network_id)));
    }

    #[test]
    fn explicit_deny_wins_over_allow() {
        let mut input = base_input(NetworkRole::Owner);
        input.network_acl = Some(acl(
            &input,
            vec![
                rule(b"allow-read", PermissionEffect::Allow),
                rule(b"deny-read", PermissionEffect::Deny),
            ],
        ));
        let result = input.evaluate();
        assert!(!result.decision.allowed);
        assert_eq!(result.decision.reason, EffectiveAccessReason::ExplicitDeny);
        assert_eq!(
            result.decision.rule_id,
            Some(SecurityRuleId::from_public_key(b"deny-read"))
        );
        assert!(result
            .decision
            .matched_allow_rule_ids
            .contains(&SecurityRuleId::from_public_key(b"allow-read")));
        let allow = result
            .explanations
            .iter()
            .find(|entry| {
                entry
                    .rule_id
                    .as_ref()
                    .is_some_and(|id| id == &SecurityRuleId::from_public_key(b"allow-read"))
            })
            .expect("allow explanation");
        assert!(allow.matched);
        assert!(!allow.applied);
    }

    #[test]
    fn role_ceiling_explains_why_a_matching_allow_was_not_applied() {
        let mut input = base_input(NetworkRole::Member);
        input.request.action = PolicyAction::Create;
        input.request.resource = ResourceKind::SecurityPolicy;
        input.request.intent = PermissionIntent::Mutating;
        let result = input.evaluate();
        assert_eq!(
            result.decision.reason,
            EffectiveAccessReason::RoleCeilingExceeded
        );
        assert!(result.explanations.iter().any(|entry| {
            entry.source == PolicyRuleSource::RoleBaseline
                && entry.matched
                && !entry.applied
                && entry.precedence == PolicyPrecedence::RoleCeilingExceeded
        }));
        assert!(result.precedence.iter().any(|entry| {
            entry.precedence == PolicyPrecedence::RoleCeilingExceeded && entry.triggered
        }));
    }

    #[test]
    fn local_veto_overrides_network_allow() {
        let mut input = base_input(NetworkRole::Owner);
        input.request = PolicyTestRequest {
            action: PolicyAction::Use,
            resource: ResourceKind::ExitGrant,
            resource_network_id: input.network_id.clone(),
            object_id: None,
            intent: PermissionIntent::Mutating,
        };
        input.network_acl = Some(NetworkAcl {
            network_id: input.network_id.clone(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            rules: Vec::new(),
            grants: vec![NetworkAclGrant {
                grant_id: SecurityRuleId::from_public_key(b"exit-grant"),
                subjects: vec![SubjectSelector::AnyMember],
                resources: vec![ResourceKind::ExitGrant],
                actions: vec![PolicyAction::Use],
                issued_at_unix_ms: 100,
                expires_at_unix_ms: 10_000,
                revoked_at_unix_ms: None,
                revoked_by: None,
                requires_recent_reauthentication: false,
            }],
            relay_limits: Default::default(),
        });
        input.local_policy = Some(NodeLocalAccessPolicy {
            node_id: actor(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            network_scope: NodeNetworkScope::All,
            contribution_vetoes: vec![NodeContributionVeto {
                veto_id: SecurityRuleId::from_public_key(b"exit-veto"),
                network_id: input.network_id.clone(),
                contributions: vec![ContributionKind::Exit],
            }],
            exit_contribution: None,
            relay_limits: Default::default(),
            rules: Vec::new(),
        });
        let result = input.evaluate();
        assert!(!result.decision.allowed);
        assert_eq!(
            result.decision.reason,
            EffectiveAccessReason::LocalContributionVeto
        );
        assert_eq!(
            result.decision.rule_id,
            Some(SecurityRuleId::from_public_key(b"exit-veto"))
        );
        assert!(result.explanations.iter().any(|entry| {
            entry.source == PolicyRuleSource::LocalContributionVeto
                && entry.matched
                && entry.applied
                && entry.precedence == PolicyPrecedence::LocalContributionVeto
        }));
    }

    #[test]
    fn temporary_coordinator_write_is_read_only_even_with_allow() {
        let mut input = base_input(NetworkRole::TemporaryCoordinator);
        input.request.action = PolicyAction::Update;
        input.request.intent = PermissionIntent::Mutating;
        let mut allow_update = rule(b"allow-update", PermissionEffect::Allow);
        allow_update.actions = vec![PolicyAction::Update];
        input.network_acl = Some(acl(&input, vec![allow_update]));
        let result = input.evaluate();
        assert!(!result.decision.allowed);
        assert_eq!(
            result.decision.reason,
            EffectiveAccessReason::TemporaryCoordinatorReadOnly
        );
        assert!(result.precedence.iter().any(|entry| entry.precedence
            == PolicyPrecedence::TemporaryCoordinatorReadOnly
            && entry.triggered));
        let allow = result
            .explanations
            .iter()
            .find(|entry| {
                entry
                    .rule_id
                    .as_ref()
                    .is_some_and(|id| id == &SecurityRuleId::from_public_key(b"allow-update"))
            })
            .expect("allow explanation");
        assert!(allow.matched);
        assert!(!allow.applied);
    }

    #[test]
    fn recent_reauthentication_grant_is_reported() {
        let mut input = base_input(NetworkRole::Owner);
        input.membership_active = false;
        input.recent_reauthentication = false;
        input.network_acl = Some(NetworkAcl {
            network_id: input.network_id.clone(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            rules: Vec::new(),
            grants: vec![NetworkAclGrant {
                grant_id: SecurityRuleId::from_public_key(b"fresh-grant"),
                subjects: vec![SubjectSelector::AnyMember],
                resources: vec![ResourceKind::Network],
                actions: vec![PolicyAction::Read],
                issued_at_unix_ms: 100,
                expires_at_unix_ms: 10_000,
                revoked_at_unix_ms: None,
                revoked_by: None,
                requires_recent_reauthentication: true,
            }],
            relay_limits: Default::default(),
        });
        let result = input.evaluate();
        assert!(!result.decision.allowed);
        assert_eq!(
            result.decision.reason,
            EffectiveAccessReason::RecentReauthenticationRequired
        );
        assert_eq!(
            result.decision.reauthentication_required_grant_ids,
            vec![SecurityRuleId::from_public_key(b"fresh-grant")]
        );
        assert!(result.explanations.iter().any(|entry| {
            entry.source == PolicyRuleSource::NetworkAclGrant
                && entry.matched
                && !entry.applied
                && entry.precedence == PolicyPrecedence::RecentReauthenticationRequired
        }));
        assert!(result.precedence.iter().any(|entry| {
            entry.precedence == PolicyPrecedence::RecentReauthenticationRequired && entry.triggered
        }));
    }

    #[test]
    fn revoked_grant_has_its_own_precedence_and_is_not_applied() {
        let mut input = base_input(NetworkRole::Owner);
        input.membership_active = false;
        input.network_acl = Some(NetworkAcl {
            network_id: input.network_id.clone(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            rules: Vec::new(),
            grants: vec![NetworkAclGrant {
                grant_id: SecurityRuleId::from_public_key(b"revoked-grant"),
                subjects: vec![SubjectSelector::AnyMember],
                resources: vec![ResourceKind::Network],
                actions: vec![PolicyAction::Read],
                issued_at_unix_ms: 100,
                expires_at_unix_ms: 10_000,
                revoked_at_unix_ms: Some(500),
                revoked_by: Some(actor()),
                requires_recent_reauthentication: false,
            }],
            relay_limits: Default::default(),
        });
        let result = input.evaluate();
        assert_eq!(
            result.decision.reason,
            EffectiveAccessReason::AuthorizationRevoked
        );
        assert!(result.explanations.iter().any(|entry| {
            entry.source == PolicyRuleSource::NetworkAclGrant
                && entry.matched
                && !entry.applied
                && entry.precedence == PolicyPrecedence::AuthorizationRevoked
        }));
        assert!(result.precedence.iter().any(|entry| {
            entry.precedence == PolicyPrecedence::AuthorizationRevoked && entry.triggered
        }));
    }

    #[test]
    fn network_scope_restriction_is_explained_independently() {
        let mut input = base_input(NetworkRole::Owner);
        input.local_policy = Some(NodeLocalAccessPolicy {
            node_id: actor(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            network_scope: NodeNetworkScope::Only(vec![network_id(b"another-network")]),
            contribution_vetoes: Vec::new(),
            exit_contribution: None,
            relay_limits: Default::default(),
            rules: Vec::new(),
        });
        let result = input.evaluate();
        assert_eq!(
            result.decision.reason,
            EffectiveAccessReason::NetworkScopeRestricted
        );
        assert!(result.explanations.iter().any(|entry| {
            entry.source == PolicyRuleSource::LocalNetworkScope && entry.matched && entry.applied
        }));
        assert!(result.precedence.iter().any(|entry| {
            entry.precedence == PolicyPrecedence::NetworkScopeRestricted && entry.triggered
        }));
    }

    #[test]
    fn validation_issues_do_not_override_an_authoritative_allow() {
        let mut input = base_input(NetworkRole::Member);
        input.emergency_coordinator = true;
        let result = input.evaluate();
        assert!(result.decision.allowed);
        assert!(!result.validation_issues.is_empty());
        assert!(result
            .precedence
            .iter()
            .any(|entry| { entry.precedence == PolicyPrecedence::Allow && entry.triggered }));
        assert!(!result.precedence.iter().any(|entry| {
            entry.precedence == PolicyPrecedence::InvalidOrExpiredPolicy && entry.triggered
        }));
    }

    #[test]
    fn validation_reports_field_paths_and_conflicting_sources() {
        let mut input = base_input(NetworkRole::Member);
        let duplicate = SecurityRuleId::from_public_key(b"duplicate");
        let mut invalid_rule = rule(b"duplicate", PermissionEffect::Deny);
        invalid_rule.subjects.clear();
        input.network_acl = Some(NetworkAcl {
            network_id: input.network_id.clone(),
            revision: 1,
            issued_at_unix_ms: 100,
            expires_at_unix_ms: 10_000,
            rules: vec![invalid_rule, rule(b"duplicate", PermissionEffect::Allow)],
            grants: vec![NetworkAclGrant {
                grant_id: duplicate.clone(),
                subjects: vec![SubjectSelector::AnyMember],
                resources: vec![ResourceKind::Network],
                actions: vec![PolicyAction::Read],
                issued_at_unix_ms: 100,
                expires_at_unix_ms: 10_000,
                revoked_at_unix_ms: None,
                revoked_by: None,
                requires_recent_reauthentication: false,
            }],
            relay_limits: Default::default(),
        });
        let result = input.evaluate();
        assert!(result.validation_issues.iter().any(|issue| {
            issue.path == "/network_acl/rules/0/subjects" && issue.code == "empty_rule_subjects"
        }));
        assert!(result.validation_issues.iter().any(|issue| {
            issue.path == "/network_acl/rules/1/rule_id"
                && issue.code == "duplicate_network_acl_rule_id"
        }));
        assert!(result.validation_issues.iter().any(|issue| {
            issue.path == "/network_acl/grants/0/grant_id"
                && issue.code == "rule_grant_id_conflict"
                && issue.related_rule_id.as_ref() == Some(&duplicate)
        }));
        assert_eq!(result.decision.reason, EffectiveAccessReason::InvalidPolicy);
    }

    #[test]
    fn evaluation_has_no_mutation_side_effects() {
        let mut input = base_input(NetworkRole::Owner);
        input.network_acl = Some(acl(&input, vec![rule(b"allow", PermissionEffect::Allow)]));
        let before = input.clone();
        let first = input.evaluate();
        let second = input.evaluate();
        assert_eq!(input, before);
        assert_eq!(first, second);
        assert!(first.decision.allowed);
    }

    #[test]
    fn advanced_json_reports_unknown_and_invalid_field_paths() {
        let mut value = serde_json::to_value(base_input(NetworkRole::Member)).expect("input json");
        value
            .as_object_mut()
            .expect("object")
            .insert("unexpected".into(), serde_json::Value::Bool(true));
        let issue = parse_policy_test_input(&value.to_string()).expect_err("unknown field");
        assert_eq!(issue.path, "/unexpected");
        assert_eq!(issue.code, "invalid_test_input_json");
        assert_eq!(issue.source, PolicyValidationSource::TestInput);

        let mut value = serde_json::to_value(base_input(NetworkRole::Member)).expect("input json");
        value["membership_active"] = serde_json::Value::String("yes".into());
        let issue = parse_policy_test_input(&value.to_string()).expect_err("invalid type");
        assert_eq!(issue.path, "/membership_active");
        assert!(!issue.message.is_empty());
    }

    #[test]
    fn advanced_json_rejects_duplicate_fields_at_each_nesting_level() {
        let json = serde_json::to_string(&base_input(NetworkRole::Member)).expect("input json");
        let duplicate = json.replacen(
            "\"membership_active\":true",
            "\"membership_active\":true,\"membership_active\":false",
            1,
        );
        let issue = parse_policy_test_input(&duplicate).expect_err("top-level duplicate");
        assert_eq!(issue.path, "/membership_active");
        assert_eq!(issue.code, "invalid_test_input_json");

        let json = serde_json::to_string(&base_input(NetworkRole::Member)).expect("input json");
        let duplicate = json.replacen(
            "\"intent\":\"read_only\"",
            "\"intent\":\"read_only\",\"intent\":\"mutating\"",
            1,
        );
        let issue = parse_policy_test_input(&duplicate).expect_err("nested duplicate");
        assert_eq!(issue.path, "/request/intent");
    }

    #[test]
    fn advanced_json_rejects_trailing_input_and_reports_array_paths() {
        let json = serde_json::to_string(&base_input(NetworkRole::Member)).expect("input json");
        let issue = parse_policy_test_input(&format!("{json} {{")).expect_err("trailing input");
        assert_eq!(issue.path, "/");
        assert_eq!(issue.code, "invalid_test_input_json");

        let mut value = serde_json::to_value(base_input(NetworkRole::Member)).expect("input json");
        value["groups"] = serde_json::json!([1]);
        let issue = parse_policy_test_input(&value.to_string()).expect_err("invalid array item");
        assert_eq!(issue.path, "/groups/0");
        assert_eq!(issue.code, "invalid_test_input_json");
    }
}
