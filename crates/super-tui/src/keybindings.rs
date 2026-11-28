//! App-level keybindings: defaults plus `~/.super/agent/keybindings.json`
//! overrides mapping action names to key specs.
//!
//! A key usually has one action. A key may also carry an action that acts
//! only while it has something to act on, before the key's other action:
//! stopping the background tasks has Ctrl+B to itself by default, and a
//! user who binds it to a key that has an action gets both on that key.

use crate::keys::KeyEvent;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Submit,
    Newline,
    QueueFollowUp,
    Dequeue,
    Abort,
    Quit,
    CycleModel,
    CycleModelBackward,
    SelectModel,
    SaveDefault,
    CycleThinking,
    ToggleThinking,
    StopBackgroundTasks,
    CopyLastResponse,
    OpenTree,
    ExternalEditor,
    HistoryPrev,
    HistoryNext,
    ExpandTools,
    Clear,
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Submit => "submit",
            Action::Newline => "newline",
            Action::QueueFollowUp => "queueFollowUp",
            Action::Dequeue => "dequeue",
            Action::Abort => "abort",
            Action::Quit => "quit",
            Action::CycleModel => "cycleModel",
            Action::CycleModelBackward => "cycleModelBackward",
            Action::SelectModel => "selectModel",
            Action::SaveDefault => "saveDefault",
            Action::CycleThinking => "cycleThinking",
            Action::ToggleThinking => "toggleThinking",
            Action::StopBackgroundTasks => "stopBackgroundTasks",
            Action::CopyLastResponse => "copyLastResponse",
            Action::OpenTree => "openTree",
            Action::ExternalEditor => "externalEditor",
            Action::HistoryPrev => "historyPrev",
            Action::HistoryNext => "historyNext",
            Action::ExpandTools => "expandTools",
            Action::Clear => "clear",
        }
    }

    /// Whether the action acts only while it has something to act on, and
    /// otherwise leaves its key to the key's other action.
    pub fn is_conditional(&self) -> bool {
        matches!(self, Action::StopBackgroundTasks)
    }

    fn all() -> &'static [Action] {
        &[
            Action::Submit,
            Action::Newline,
            Action::QueueFollowUp,
            Action::Dequeue,
            Action::Abort,
            Action::Quit,
            Action::CycleModel,
            Action::CycleModelBackward,
            Action::SelectModel,
            Action::SaveDefault,
            Action::CycleThinking,
            Action::ToggleThinking,
            Action::StopBackgroundTasks,
            Action::CopyLastResponse,
            Action::OpenTree,
            Action::ExternalEditor,
            Action::HistoryPrev,
            Action::HistoryNext,
            Action::ExpandTools,
            Action::Clear,
        ]
    }

    fn default_spec(&self) -> &'static str {
        match self {
            Action::Submit => "enter",
            Action::Newline => "shift+enter",
            Action::QueueFollowUp => "ctrl+enter",
            Action::Dequeue => "alt+up",
            Action::Abort => "escape",
            Action::Quit => "ctrl+d",
            Action::CycleModel => "ctrl+p",
            Action::CycleModelBackward => "shift+ctrl+p",
            Action::SelectModel => "ctrl+l",
            Action::SaveDefault => "ctrl+s",
            Action::CycleThinking => "shift+tab",
            Action::ToggleThinking => "ctrl+t",
            Action::StopBackgroundTasks => "ctrl+b",
            Action::CopyLastResponse => "ctrl+x",
            Action::OpenTree => "",
            Action::ExternalEditor => "ctrl+g",
            Action::HistoryPrev => "up",
            Action::HistoryNext => "down",
            Action::ExpandTools => "ctrl+r",
            Action::Clear => "ctrl+c",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Keybindings {
    /// The actions of each key, in the order the terminal tries them.
    map: HashMap<KeyEvent, Vec<Action>>,
}

impl Default for Keybindings {
    fn default() -> Self {
        let mut map: HashMap<KeyEvent, Vec<Action>> = HashMap::new();
        for action in Action::all() {
            if let Some(key) = KeyEvent::parse(action.default_spec()) {
                map.entry(key).or_default().push(*action);
            }
        }
        Keybindings { map }
    }
}

impl Keybindings {
    /// Apply user overrides: {"cycleModel": "ctrl+m", ...}.
    pub fn load_overrides(&mut self) {
        let Some(path) = dirs::home_dir().map(|h| h.join(".super/agent/keybindings.json")) else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(overrides) = serde_json::from_str::<HashMap<String, String>>(&text) else {
            return;
        };
        for (action_name, spec) in overrides {
            let Some(action) = Action::all()
                .iter()
                .find(|a| a.name() == action_name)
                .copied()
            else {
                continue;
            };
            let Some(key) = KeyEvent::parse(&spec) else {
                continue;
            };
            self.bind(key, action);
        }
    }

    /// Bind `action` to `key`: the action leaves the key it had. An action
    /// that acts only now and then goes first on the key and keeps the
    /// key's other action; any other action takes the key from the one it
    /// had.
    pub fn bind(&mut self, key: KeyEvent, action: Action) {
        for actions in self.map.values_mut() {
            actions.retain(|mapped| *mapped != action);
        }
        let actions = self.map.entry(key).or_default();
        if action.is_conditional() {
            actions.retain(|mapped| !mapped.is_conditional());
            actions.insert(0, action);
        } else {
            actions.retain(|mapped| mapped.is_conditional());
            actions.push(action);
        }
        self.map.retain(|_, actions| !actions.is_empty());
    }

    /// The first action of `key`.
    pub fn action_for(&self, key: &KeyEvent) -> Option<Action> {
        self.actions_for(key).first().copied()
    }

    /// Every action of `key`, in the order the terminal tries them.
    pub fn actions_for(&self, key: &KeyEvent) -> &[Action] {
        self.map.get(key).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn key_for(&self, action: Action) -> Option<&KeyEvent> {
        self.map
            .iter()
            .find_map(|(key, mapped)| mapped.contains(&action).then_some(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve() {
        let kb = Keybindings::default();
        assert_eq!(
            kb.action_for(&KeyEvent::parse("enter").unwrap()),
            Some(Action::Submit)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("ctrl+p").unwrap()),
            Some(Action::CycleModel)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("shift+ctrl+p").unwrap()),
            Some(Action::CycleModelBackward)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("shift+tab").unwrap()),
            Some(Action::CycleThinking)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("ctrl+s").unwrap()),
            Some(Action::SaveDefault)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("ctrl+d").unwrap()),
            Some(Action::Quit)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("shift+enter").unwrap()),
            Some(Action::Newline)
        );
        assert_eq!(
            kb.action_for(&KeyEvent::parse("ctrl+enter").unwrap()),
            Some(Action::QueueFollowUp)
        );
        assert_eq!(kb.action_for(&KeyEvent::parse("alt+enter").unwrap()), None);
        assert_eq!(
            kb.action_for(&KeyEvent::parse("ctrl+r").unwrap()),
            Some(Action::ExpandTools)
        );
        assert_eq!(
            kb.key_for(Action::SaveDefault).map(ToString::to_string),
            Some("ctrl+s".into())
        );
    }

    #[test]
    fn ctrl_b_stops_background_tasks_and_shares_a_key_only_when_bound_to_one() {
        let ctrl_b = KeyEvent::parse("ctrl+b").unwrap();
        let ctrl_x = KeyEvent::parse("ctrl+x").unwrap();
        let mut kb = Keybindings::default();
        assert_eq!(kb.actions_for(&ctrl_b), [Action::StopBackgroundTasks]);
        assert_eq!(kb.actions_for(&ctrl_x), [Action::CopyLastResponse]);
        assert_eq!(kb.key_for(Action::StopBackgroundTasks), Some(&ctrl_b));
        assert_eq!(kb.key_for(Action::CopyLastResponse), Some(&ctrl_x));
        assert_eq!(Action::StopBackgroundTasks.name(), "stopBackgroundTasks");

        // Binding it where it is changes nothing.
        kb.bind(ctrl_b.clone(), Action::StopBackgroundTasks);
        assert_eq!(kb.actions_for(&ctrl_b), [Action::StopBackgroundTasks]);

        // On a key that has an action it goes first, and the key keeps
        // its other action.
        kb.bind(ctrl_x.clone(), Action::StopBackgroundTasks);
        assert_eq!(
            kb.actions_for(&ctrl_x),
            [Action::StopBackgroundTasks, Action::CopyLastResponse]
        );
        assert_eq!(kb.action_for(&ctrl_x), Some(Action::StopBackgroundTasks));
        assert!(kb.actions_for(&ctrl_b).is_empty());

        // Back on a key of its own, the other key copies only.
        kb.bind(ctrl_b.clone(), Action::StopBackgroundTasks);
        assert_eq!(kb.actions_for(&ctrl_b), [Action::StopBackgroundTasks]);
        assert_eq!(kb.actions_for(&ctrl_x), [Action::CopyLastResponse]);

        // Another action on its key comes after it.
        kb.bind(ctrl_b.clone(), Action::CycleModel);
        assert_eq!(
            kb.actions_for(&ctrl_b),
            [Action::StopBackgroundTasks, Action::CycleModel]
        );
        assert_eq!(kb.action_for(&KeyEvent::parse("ctrl+p").unwrap()), None);

        // An action that takes a key from another leaves it without one.
        kb.bind(ctrl_x.clone(), Action::Quit);
        assert_eq!(kb.actions_for(&ctrl_x), [Action::Quit]);
        assert_eq!(kb.key_for(Action::CopyLastResponse), None);
    }
}
