use alloc::string::String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Cards,
    Actions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Menu,
    Diagnostics,
    Error,
}

#[derive(Clone, Debug)]
pub struct MenuState {
    pub selected: usize,
    pub focus: Focus,
    pub action: usize,
    pub dark: bool,
    pub countdown_ms: Option<u32>,
    pub countdown_total_ms: u32,
    pub screen: Screen,
    pub message: String,
    pub cursor: Option<(u32, u32)>,
    pub anim_ms: u32,
}

impl MenuState {
    pub fn new(selected: usize, dark: bool, countdown_ms: Option<u32>) -> Self {
        Self {
            selected,
            focus: Focus::Cards,
            action: 0,
            dark,
            countdown_total_ms: countdown_ms.unwrap_or(0),
            countdown_ms,
            screen: Screen::Menu,
            message: String::new(),
            cursor: None,
            anim_ms: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Left,
    Right,
    Up,
    Down,
    Activate,
    Tab,
    Escape,
    Theme,
    Firmware,
    Reboot,
    Shutdown,
    Recovery,
    Diagnostics,
    HoverCard(usize),
    ClickCard(usize),
    ClickAction(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Boot,
    Action(usize),
    Recovery,
    Exit,
}

pub fn step(
    state: &mut MenuState,
    input: Option<Input>,
    n_cards: usize,
    cols: usize,
    dt_ms: u32,
) -> Outcome {
    if n_cards == 0 {
        state.selected = 0;
    } else if state.selected >= n_cards {
        state.selected = n_cards - 1;
    }
    let cols = cols.max(1);
    if let Some(input) = input {
        if !matches!(input, Input::HoverCard(_)) {
            state.countdown_ms = None;
        }
        if state.screen != Screen::Menu {
            match input {
                Input::Escape | Input::Activate => state.screen = Screen::Menu,
                Input::Theme => state.dark = !state.dark,
                _ => {}
            }
            return Outcome::None;
        }
        match input {
            Input::Left => move_selection(state, n_cards, -1),
            Input::Right => move_selection(state, n_cards, 1),
            Input::Up => {
                if state.focus == Focus::Actions {
                    state.focus = Focus::Cards;
                } else {
                    move_selection(state, n_cards, -(cols as isize));
                }
            }
            Input::Down => move_selection(state, n_cards, cols as isize),
            Input::Tab => {
                state.focus = if state.focus == Focus::Cards {
                    Focus::Actions
                } else {
                    Focus::Cards
                };
            }
            Input::Activate => {
                if state.focus == Focus::Actions {
                    return Outcome::Action(state.action);
                }
                if n_cards > 0 {
                    return Outcome::Boot;
                }
            }
            Input::Escape => return Outcome::Exit,
            Input::Theme => state.dark = !state.dark,
            Input::Firmware => return Outcome::Action(0),
            Input::Reboot => return Outcome::Action(1),
            Input::Shutdown => return Outcome::Action(2),
            Input::Recovery => return Outcome::Recovery,
            Input::Diagnostics => state.screen = Screen::Diagnostics,
            Input::HoverCard(index) => {
                if index < n_cards && index != state.selected {
                    state.selected = index;
                    state.focus = Focus::Cards;
                    state.anim_ms = 160;
                }
            }
            Input::ClickCard(index) => {
                if index < n_cards {
                    state.selected = index;
                    state.focus = Focus::Cards;
                    return Outcome::Boot;
                }
            }
            Input::ClickAction(index) => {
                state.focus = Focus::Actions;
                state.action = index;
                return Outcome::Action(index);
            }
        }
        return Outcome::None;
    }
    if state.screen == Screen::Menu {
        if let Some(ms) = state.countdown_ms {
            let left = ms.saturating_sub(dt_ms);
            if left == 0 {
                state.countdown_ms = None;
                if n_cards > 0 {
                    return Outcome::Boot;
                }
            } else {
                state.countdown_ms = Some(left);
            }
        }
        if state.anim_ms > 0 {
            state.anim_ms = state.anim_ms.saturating_sub(dt_ms);
        }
    }
    Outcome::None
}

fn move_selection(state: &mut MenuState, n_cards: usize, delta: isize) {
    if state.focus == Focus::Actions {
        let count = 3isize;
        state.action = wrap(state.action as isize + delta.signum(), count);
        return;
    }
    if n_cards == 0 {
        return;
    }
    let next = wrap(state.selected as isize + delta, n_cards as isize);
    if next != state.selected {
        state.selected = next;
        state.anim_ms = 160;
    }
}

fn wrap(value: isize, n: isize) -> usize {
    if n <= 0 {
        return 0;
    }
    (((value % n) + n) % n) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_wraps_countdown_and_theme() {
        let mut state = MenuState::new(0, true, Some(30));
        assert_eq!(step(&mut state, Some(Input::Left), 3, 1, 0), Outcome::None);
        assert_eq!(state.selected, 2);
        assert_eq!(state.countdown_ms, None);
        assert_eq!(step(&mut state, Some(Input::Theme), 3, 1, 0), Outcome::None);
        assert!(!state.dark);
        assert_eq!(
            step(&mut state, Some(Input::Activate), 3, 1, 0),
            Outcome::Boot
        );

        let mut state = MenuState::new(0, true, Some(20));
        assert_eq!(step(&mut state, None, 2, 1, 20), Outcome::Boot);

        let mut state = MenuState::new(0, true, Some(5_000));
        assert_eq!(
            step(&mut state, Some(Input::Diagnostics), 2, 1, 0),
            Outcome::None
        );
        assert_eq!(state.screen, Screen::Diagnostics);
        assert_eq!(
            step(&mut state, Some(Input::Escape), 2, 1, 0),
            Outcome::None
        );
        assert_eq!(state.screen, Screen::Menu);
        assert_eq!(step(&mut state, Some(Input::Tab), 2, 1, 0), Outcome::None);
        assert_eq!(state.focus, Focus::Actions);
        assert_eq!(
            step(&mut state, Some(Input::Activate), 2, 1, 0),
            Outcome::Action(0)
        );
    }
}
