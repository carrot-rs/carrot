//! Accessibility data model.
//!
//! Defines the role vocabulary + tree shape that the eventual
//! [`accesskit`] adapter translates to platform accessibility APIs
//! (NSAccessibility on macOS, UI Automation on Windows, AT-SPI on
//! Linux). Shipping the data types now lets the rest of the UI
//! annotate its elements with the right roles before the adapter
//! lands — once [`accesskit`] is wired up, all those annotations
//! become live.
//!
//! # Why an in-crate model
//!
//! We could depend on [`accesskit`] directly and use its `Node` type.
//! Two reasons not to:
//!
//! 1. [`accesskit`] pulls in a larger dep graph than we want today.
//! 2. Our role set is narrower than AccessKit's and we want to keep
//!    mapping decisions explicit (e.g. our `TerminalBlock` role maps
//!    to AccessKit's `Group` + a live region, not `Terminal`).
//!
//! When the adapter lands it lives in `platform/accesskit.rs` and
//! implements `From<AccessibilityRole> for accesskit::Role` plus
//! `From<AccessibilityNode> for accesskit::Node`.

use std::collections::BTreeMap;

/// Stable opaque handle for a node in the accessibility tree. Maps
/// 1:1 to `accesskit::NodeId` when the adapter translates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccessibilityId(pub u64);

/// Semantic role of a UI element. The set is intentionally small —
/// each role maps to a well-defined AccessKit role in the future
/// adapter. Extend deliberately, not opportunistically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessibilityRole {
    /// Generic container with no semantics beyond layout.
    Group,
    /// Top-level window root.
    Window,
    /// Menu bar / title bar strip.
    TitleBar,
    /// Terminal block (shell / TUI output). Announced as a live
    /// region so screen readers read new content as it arrives.
    TerminalBlock,
    /// Editable text region (cmdline input, editor).
    TextInput,
    /// Static text — labels, headings.
    Label,
    /// A push button.
    Button,
    /// A clickable list entry (tab, file row, command-palette item).
    ListItem,
    /// Container for list items.
    List,
    /// Scrollable viewport (the block list's outer region).
    Scrollable,
    /// Status-line region (bottom strip).
    StatusLine,
    /// Dialog / modal surface.
    Dialog,
    /// Tooltip / transient popover.
    Tooltip,
}

/// Live-region politeness for roles that announce updates to screen
/// readers. Matches the ARIA live-region vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LivePoliteness {
    /// Don't announce updates.
    #[default]
    Off,
    /// Wait for the user to pause, then announce.
    Polite,
    /// Announce immediately (use sparingly — errors, critical alerts).
    Assertive,
}

/// A single node in the accessibility tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccessibilityNode {
    /// Stable identifier used by parents to reference this node.
    pub id: AccessibilityId,
    /// Semantic role exposed to assistive technology.
    pub role: AccessibilityRole,
    /// Accessible name — what a screen reader announces. For a
    /// terminal block this is typically the command string; for a
    /// button it's the label.
    pub name: Option<String>,
    /// Supplementary description (tooltip text, help string).
    pub description: Option<String>,
    /// Current value for editable text / numeric roles.
    pub value: Option<String>,
    /// For live regions: how aggressively updates are announced.
    pub live: LivePoliteness,
    /// Child ids in visual order.
    pub children: Vec<AccessibilityId>,
    /// Whether the node is currently focused.
    pub focused: bool,
    /// Whether the node accepts user input.
    pub disabled: bool,
}

impl AccessibilityNode {
    /// Creates a new node with the given id and role. All optional
    /// attributes default to `None`/`false` and can be layered on via
    /// the `with_*` builders.
    pub fn new(id: AccessibilityId, role: AccessibilityRole) -> Self {
        Self {
            id,
            role,
            name: None,
            description: None,
            value: None,
            live: LivePoliteness::default(),
            children: Vec::new(),
            focused: false,
            disabled: false,
        }
    }

    /// Sets the accessible name announced by screen readers.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attaches a supplementary description (tooltip / help text).
    pub fn with_description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    /// Attaches the current value — used for editable text or numeric
    /// roles where the value is announced separately from the name.
    pub fn with_value(mut self, v: impl Into<String>) -> Self {
        self.value = Some(v.into());
        self
    }

    /// Sets the live-region politeness for roles that announce updates.
    pub fn with_live(mut self, live: LivePoliteness) -> Self {
        self.live = live;
        self
    }

    /// Replaces the child id list. Ordering is significant — assistive
    /// technology announces children in the order given.
    pub fn with_children(mut self, children: Vec<AccessibilityId>) -> Self {
        self.children = children;
        self
    }

    /// Marks the node as currently focused (or clears the flag).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Marks the node as disabled (or clears the flag). Disabled nodes
    /// are still announced but indicated as non-interactive.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Flat map of the accessibility tree. Each node references its
/// children by id; the consumer walks from the root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessibilityTree {
    /// Identifier of the root node. `None` until a node is inserted.
    pub root: Option<AccessibilityId>,
    /// All nodes in the tree, keyed by id. Parent/child links are
    /// expressed via [`AccessibilityNode::children`], not via this map.
    pub nodes: BTreeMap<AccessibilityId, AccessibilityNode>,
}

impl AccessibilityTree {
    /// Creates an empty tree with no root node.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a node. The first node inserted into an empty tree
    /// becomes the root; subsequent nodes must be linked via an
    /// existing parent's [`AccessibilityNode::children`].
    pub fn insert(&mut self, node: AccessibilityNode) {
        let id = node.id;
        if self.root.is_none() {
            self.root = Some(id);
        }
        self.nodes.insert(id, node);
    }

    /// Removes the node with the given id and returns it. Clears the
    /// root pointer if the removed node was the root.
    pub fn remove(&mut self, id: AccessibilityId) -> Option<AccessibilityNode> {
        if self.root == Some(id) {
            self.root = None;
        }
        self.nodes.remove(&id)
    }

    /// Returns a reference to the node with the given id, if it exists.
    pub fn get(&self, id: AccessibilityId) -> Option<&AccessibilityNode> {
        self.nodes.get(&id)
    }

    /// Number of nodes in the tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` when the tree contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Overrides the root pointer. The referenced node does not need to
    /// be present yet — callers may set the root before inserting the
    /// corresponding node.
    pub fn set_root(&mut self, id: AccessibilityId) {
        self.root = Some(id);
    }

    /// Depth-first iterator rooted at `self.root`. Skips children
    /// that reference missing nodes (defensive — stale removals).
    pub fn walk(&self) -> Vec<&AccessibilityNode> {
        let mut out = Vec::new();
        if let Some(root_id) = self.root {
            self.walk_from(root_id, &mut out);
        }
        out
    }

    fn walk_from<'a>(&'a self, id: AccessibilityId, out: &mut Vec<&'a AccessibilityNode>) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        out.push(node);
        for child_id in &node.children {
            self.walk_from(*child_id, out);
        }
    }
}

// ─── AccessKit adapter ───────────────────────────────────────────

/// Translate a Carrot [`AccessibilityRole`] to an [`accesskit::Role`].
///
/// The mapping picks the closest AccessKit variant; our narrower
/// role set is deliberate — keeping the mapping explicit here means
/// consumers don't have to re-interpret AccessKit's richer
/// vocabulary for the handful of roles we actually emit.
pub fn role_to_accesskit(role: AccessibilityRole) -> accesskit::Role {
    match role {
        AccessibilityRole::Group => accesskit::Role::GenericContainer,
        AccessibilityRole::Window => accesskit::Role::Window,
        AccessibilityRole::TitleBar => accesskit::Role::TitleBar,
        AccessibilityRole::TerminalBlock => accesskit::Role::Terminal,
        AccessibilityRole::TextInput => accesskit::Role::TextInput,
        AccessibilityRole::Label => accesskit::Role::Label,
        AccessibilityRole::Button => accesskit::Role::Button,
        AccessibilityRole::ListItem => accesskit::Role::ListItem,
        AccessibilityRole::List => accesskit::Role::List,
        AccessibilityRole::Scrollable => accesskit::Role::ScrollView,
        AccessibilityRole::StatusLine => accesskit::Role::Status,
        AccessibilityRole::Dialog => accesskit::Role::Dialog,
        AccessibilityRole::Tooltip => accesskit::Role::Tooltip,
    }
}

/// Translate a Carrot [`LivePoliteness`] to an [`accesskit::Live`].
pub fn live_to_accesskit(live: LivePoliteness) -> accesskit::Live {
    match live {
        LivePoliteness::Off => accesskit::Live::Off,
        LivePoliteness::Polite => accesskit::Live::Polite,
        LivePoliteness::Assertive => accesskit::Live::Assertive,
    }
}

/// Build a fresh [`accesskit::Node`] from a Carrot node. Child
/// relationships are applied by the caller via the AccessKit tree
/// API — this helper fills in the single-node role + name +
/// description + value + live-region + state bits.
pub fn node_to_accesskit(node: &AccessibilityNode) -> accesskit::Node {
    let mut ak = accesskit::Node::new(role_to_accesskit(node.role));
    if let Some(name) = &node.name {
        ak.set_label(name.clone());
    }
    if let Some(description) = &node.description {
        ak.set_description(description.clone());
    }
    if let Some(value) = &node.value {
        ak.set_value(value.clone());
    }
    ak.set_live(live_to_accesskit(node.live));
    if node.disabled {
        ak.set_disabled();
    }
    ak
}

/// Map a Carrot [`AccessibilityId`] to an [`accesskit::NodeId`].
/// AccessKit's `NodeId` is a u64 newtype — a 1:1 map.
pub fn id_to_accesskit(id: AccessibilityId) -> accesskit::NodeId {
    accesskit::NodeId(id.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> AccessibilityId {
        AccessibilityId(n)
    }

    #[test]
    fn node_builder_chains_apply() {
        let n = AccessibilityNode::new(id(1), AccessibilityRole::Button)
            .with_name("Run")
            .with_description("Submit the current command")
            .with_value("")
            .with_live(LivePoliteness::Polite)
            .focused(true)
            .disabled(false);
        assert_eq!(n.name.as_deref(), Some("Run"));
        assert_eq!(n.description.as_deref(), Some("Submit the current command"));
        assert!(matches!(n.live, LivePoliteness::Polite));
        assert!(n.focused);
    }

    #[test]
    fn tree_insert_sets_root_from_first_node() {
        let mut tree = AccessibilityTree::new();
        tree.insert(AccessibilityNode::new(id(42), AccessibilityRole::Window));
        assert_eq!(tree.root, Some(id(42)));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn tree_get_returns_inserted_node() {
        let mut tree = AccessibilityTree::new();
        tree.insert(AccessibilityNode::new(id(1), AccessibilityRole::Group));
        let got = tree.get(id(1)).unwrap();
        assert_eq!(got.role, AccessibilityRole::Group);
    }

    #[test]
    fn tree_remove_clears_root_when_removing_root() {
        let mut tree = AccessibilityTree::new();
        tree.insert(AccessibilityNode::new(id(1), AccessibilityRole::Window));
        tree.remove(id(1));
        assert!(tree.root.is_none());
        assert!(tree.is_empty());
    }

    #[test]
    fn walk_returns_depth_first_order() {
        let mut tree = AccessibilityTree::new();
        tree.insert(
            AccessibilityNode::new(id(1), AccessibilityRole::Window)
                .with_children(vec![id(2), id(3)]),
        );
        tree.insert(
            AccessibilityNode::new(id(2), AccessibilityRole::Group).with_children(vec![id(4)]),
        );
        tree.insert(AccessibilityNode::new(id(3), AccessibilityRole::Group));
        tree.insert(AccessibilityNode::new(id(4), AccessibilityRole::Button));
        let order: Vec<_> = tree.walk().iter().map(|n| n.id.0).collect();
        assert_eq!(order, vec![1, 2, 4, 3]);
    }

    #[test]
    fn walk_skips_missing_children() {
        let mut tree = AccessibilityTree::new();
        tree.insert(
            AccessibilityNode::new(id(1), AccessibilityRole::Window)
                .with_children(vec![id(2), id(99)]),
        );
        tree.insert(AccessibilityNode::new(id(2), AccessibilityRole::Group));
        let ids: Vec<_> = tree.walk().iter().map(|n| n.id.0).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn live_politeness_defaults_to_off() {
        let p = LivePoliteness::default();
        assert!(matches!(p, LivePoliteness::Off));
    }

    #[test]
    fn empty_tree_walk_returns_empty() {
        let tree = AccessibilityTree::new();
        assert!(tree.walk().is_empty());
    }

    #[test]
    fn terminal_block_role_is_stable() {
        // Regression guard: the adapter relies on this exact
        // variant to mark the node as a live region.
        let n = AccessibilityNode::new(id(1), AccessibilityRole::TerminalBlock)
            .with_live(LivePoliteness::Polite);
        assert!(matches!(n.role, AccessibilityRole::TerminalBlock));
        assert!(matches!(n.live, LivePoliteness::Polite));
    }

    #[test]
    fn node_equality_is_full_value_compare() {
        let a = AccessibilityNode::new(id(1), AccessibilityRole::Button).with_name("ok");
        let b = AccessibilityNode::new(id(1), AccessibilityRole::Button).with_name("ok");
        assert_eq!(a, b);
    }

    #[test]
    fn tree_preserves_explicit_root_override() {
        let mut tree = AccessibilityTree::new();
        tree.insert(AccessibilityNode::new(id(1), AccessibilityRole::Group));
        tree.insert(AccessibilityNode::new(id(2), AccessibilityRole::Window));
        // Switch root to the Window node.
        tree.set_root(id(2));
        assert_eq!(tree.root, Some(id(2)));
    }

    // ─── AccessKit adapter tests ──────────────────────────────

    #[test]
    fn role_maps_every_variant_to_accesskit() {
        // Regression guard: if a variant is added, this match
        // forces the adapter to pick its accesskit counterpart.
        for role in [
            AccessibilityRole::Group,
            AccessibilityRole::Window,
            AccessibilityRole::TitleBar,
            AccessibilityRole::TerminalBlock,
            AccessibilityRole::TextInput,
            AccessibilityRole::Label,
            AccessibilityRole::Button,
            AccessibilityRole::ListItem,
            AccessibilityRole::List,
            AccessibilityRole::Scrollable,
            AccessibilityRole::StatusLine,
            AccessibilityRole::Dialog,
            AccessibilityRole::Tooltip,
        ] {
            let _ak: accesskit::Role = role_to_accesskit(role);
        }
    }

    #[test]
    fn terminal_block_maps_to_accesskit_terminal() {
        assert_eq!(
            role_to_accesskit(AccessibilityRole::TerminalBlock),
            accesskit::Role::Terminal
        );
    }

    #[test]
    fn button_maps_to_accesskit_button() {
        assert_eq!(
            role_to_accesskit(AccessibilityRole::Button),
            accesskit::Role::Button
        );
    }

    #[test]
    fn live_politeness_maps_correctly() {
        assert_eq!(live_to_accesskit(LivePoliteness::Off), accesskit::Live::Off);
        assert_eq!(
            live_to_accesskit(LivePoliteness::Polite),
            accesskit::Live::Polite
        );
        assert_eq!(
            live_to_accesskit(LivePoliteness::Assertive),
            accesskit::Live::Assertive
        );
    }

    #[test]
    fn id_maps_one_to_one_to_accesskit_nodeid() {
        assert_eq!(id_to_accesskit(id(42)), accesskit::NodeId(42));
        assert_eq!(id_to_accesskit(id(0)), accesskit::NodeId(0));
    }

    #[test]
    fn node_to_accesskit_preserves_disabled_flag() {
        let n = AccessibilityNode::new(id(1), AccessibilityRole::Button)
            .disabled(true)
            .with_name("Run");
        let ak = node_to_accesskit(&n);
        assert!(ak.is_disabled());
    }
}
