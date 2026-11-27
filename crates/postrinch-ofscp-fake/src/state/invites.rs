//! Group invites (§5.6) and the role rules they meet: which roles exist,
//! and the subset rule (§5.7) that keeps an invite from handing out more
//! than its creator holds.

use jiff::Timestamp;
use ofscp_core::wire::{Group, Invite, Role};
use sha2::{Digest, Sha256};

use super::groups::OWNER;

/// The roles every group has (§5.2), whatever its catalogue lists.
const CANONICAL_ROLES: [&str; 5] = [OWNER, "admin", "moderator", "member", "guest"];

/// An invite as the fake keeps it: what it serves, without the token,
/// and the token's SHA-256, as the spec recommends (§5.6), so the token
/// is never kept.
#[derive(Debug, Clone)]
pub(crate) struct InviteRec {
    /// The invite as listed, `token` left out.
    pub(crate) invite: Invite,
    pub(crate) digest: [u8; 32],
}

impl InviteRec {
    /// Has it expired at `now`?
    pub(crate) fn expired(&self, now: Timestamp) -> bool {
        self.invite.expires_at.is_some_and(|at| at <= now)
    }

    /// Are all its uses spent?
    pub(crate) fn exhausted(&self) -> bool {
        self.invite
            .max_uses
            .is_some_and(|max| self.invite.uses >= max)
    }
}

/// The digest an invite is found by.
pub(crate) fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// May an invite grant `role` in `group` (§5.6)? A canonical role or one
/// in the group's catalogue, never `owner`.
pub(crate) fn grantable(group: &Group, role: &str) -> bool {
    if role == OWNER {
        return false;
    }
    CANONICAL_ROLES.contains(&role)
        || group
            .roles
            .as_ref()
            .is_some_and(|roles| roles.iter().any(|r| r.name == role))
}

/// The group actions `role` holds (§5.2): every action whose list names
/// it, and every action at all for the owner.
fn permissions(group: &Group, role: &str) -> Vec<String> {
    let p = &group.permissions;
    let named = [("post", &p.post), ("moderate", &p.moderate), ("manage", &p.manage)];
    let mut actions: Vec<(String, Option<&Vec<Role>>)> = named
        .into_iter()
        .map(|(action, list)| (action.to_owned(), list.as_ref()))
        .collect();
    actions.extend(p.other.iter().map(|(a, list)| (a.clone(), Some(list))));
    actions
        .into_iter()
        .filter(|(_, list)| role == OWNER || list.is_some_and(|l| l.iter().any(|r| r == role)))
        .map(|(action, _)| action)
        .collect()
}

/// The subset rule (§5.7): does `actor_role` hold every permission
/// `target` holds?
pub(crate) fn holds_all_of(group: &Group, actor_role: &str, target: &str) -> bool {
    let held = permissions(group, actor_role);
    permissions(group, target).iter().all(|p| held.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::groups::effective_permissions;
    use ofscp_core::wire::{JoinPolicy, RoleDefinition, Tier};

    fn group() -> Group {
        Group {
            id: ofscp_core::LocalId::new("grp_1").unwrap(),
            name: "G".to_owned(),
            description: None,
            owner: ofscp_core::Actor::parse_canonical("ann@a.example").unwrap(),
            join_policy: JoinPolicy::Invite,
            tier: Tier::Group,
            permissions: effective_permissions(None),
            roles: Some(vec![RoleDefinition {
                name: "helper".to_owned(),
                label: None,
                color: None,
            }]),
            membership: None,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            metadata: Vec::new(),
        }
    }

    #[test]
    fn an_invite_grants_canonical_or_catalogued_roles_but_never_owner() {
        let g = group();
        assert!(grantable(&g, "member"));
        assert!(grantable(&g, "helper"));
        assert!(!grantable(&g, "owner"));
        assert!(!grantable(&g, "wizard"));
    }

    #[test]
    fn the_subset_rule_compares_what_each_role_holds() {
        let g = group();
        assert!(holds_all_of(&g, "admin", "member"));
        assert!(holds_all_of(&g, "owner", "admin"));
        assert!(!holds_all_of(&g, "member", "admin"));
        // A catalogued role no list names holds nothing.
        assert!(holds_all_of(&g, "member", "helper"));
    }

    #[test]
    fn the_digest_is_of_the_token() {
        assert_eq!(digest("abc"), digest("abc"));
        assert_ne!(digest("abc"), digest("abd"));
    }
}
