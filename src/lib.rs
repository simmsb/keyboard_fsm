#![cfg_attr(test, feature(assert_matches))]
#![allow(unused)]

use std::ops::RangeInclusive;

use embedded_time::duration::Milliseconds;
use embedded_time::Instant;

bitflags::bitflags! {
    struct StateFlags: u8 {
        const CTRL = 0b00001;
        const SHFT = 0b00010;
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum InputEvent {
    Press(u8),
    Depress(u8),
}

type KeyCode = u8;

#[derive(Debug, PartialEq, Eq)]
enum KeyEvent {
    Press(KeyCode),
    Depress(KeyCode),
    PressCurrent,
    DepressCurrent,
}

enum InternalEvent {
    SetGlobalState(StateFlags),
    UnsetGlobalState(StateFlags),
}

impl InternalEvent {
    fn apply<Clock: embedded_time::Clock>(&self, state: &mut GlobalState<Clock>) {
        match self {
            InternalEvent::SetGlobalState(flags) => state.flags.insert(*flags),
            InternalEvent::UnsetGlobalState(flags) => state.flags.remove(*flags),
        }
    }
}

enum TransitionCondition {
    StateSet(StateFlags),
    StateNotSet(StateFlags),
    Pressed(RangeInclusive<u8>),
    Depressed(RangeInclusive<u8>),
    ElapsedLess(Milliseconds),
    ElapsedGreater(Milliseconds),
}

impl TransitionCondition {
    const fn pressed_single(key: u8) -> Self {
        Self::Pressed(key..=key)
    }

    const fn depressed_single(key: u8) -> Self {
        Self::Depressed(key..=key)
    }

    fn evaluate(&self, elapsed: Milliseconds, key: Option<InputEvent>, state: StateFlags) -> bool {
        match (self, key) {
            (TransitionCondition::StateSet(mask), _) => state.contains(*mask),
            (TransitionCondition::StateNotSet(mask), _) => !state.contains(*mask),
            (TransitionCondition::Pressed(x), Some(InputEvent::Press(key))) => x.contains(&key),
            (TransitionCondition::Depressed(x), Some(InputEvent::Depress(key))) => x.contains(&key),
            (TransitionCondition::ElapsedLess(x), _) => {
                eprintln!("{} < {}", elapsed, x);
                &elapsed < x
            }
            (TransitionCondition::ElapsedGreater(x), _) => {
                eprintln!("{} >= {}", elapsed, x);
                &elapsed >= x
            }
            _ => false,
        }
    }
}

struct GlobalState<Clock: embedded_time::Clock> {
    flags: StateFlags,
    entered_state: Instant<Clock>,
    current_state: &'static dyn DynState,
}

impl<Clock: embedded_time::Clock> GlobalState<Clock>
where
    u32: TryFrom<Clock::T>,
{
    fn tick(&mut self, current_time: Instant<Clock>) -> &'static [KeyEvent] {
        let elapsed = current_time
            .checked_duration_since(&self.entered_state)
            .unwrap()
            .try_into()
            .unwrap();

        if let Some((key_events, internal_events, next_state)) = self
            .current_state
            .transitions()
            .iter()
            .flat_map(|t| t.evaluate(elapsed, None, self.flags))
            .next()
        {
            self.do_transition(internal_events, next_state, current_time);

            return key_events;
        }

        &[]
    }

    fn push(&mut self, current_time: Instant<Clock>, event: InputEvent) -> &'static [KeyEvent] {
        let elapsed = current_time
            .checked_duration_since(&self.entered_state)
            .unwrap()
            .try_into()
            .unwrap();

        if let Some((key_events, internal_events, next_state)) = self
            .current_state
            .transitions()
            .iter()
            .flat_map(|t| t.evaluate(elapsed, Some(event), self.flags))
            .next()
        {
            self.do_transition(internal_events, next_state, current_time);

            return key_events;
        }

        &[]
    }

    fn do_transition(
        &mut self,
        internal_events: &[InternalEvent],
        next_state: &'static dyn DynState,
        current_time: Instant<Clock>,
    ) {
        for event in internal_events {
            event.apply(self);
        }

        self.current_state = next_state;
        self.entered_state = current_time;
    }
}

struct Transition<
    const CONDITION_COUNT: usize,
    const KEY_EMIT_COUNT: usize,
    const INTERNAL_EMIT_COUNT: usize,
> {
    conditions: [TransitionCondition; CONDITION_COUNT],
    key_event_emissions: [KeyEvent; KEY_EMIT_COUNT],
    internal_event_emissions: [InternalEvent; INTERNAL_EMIT_COUNT],
    target: &'static dyn DynState,
}

impl<
        const CONDITION_COUNT: usize,
        const KEY_EMIT_COUNT: usize,
        const INTERNAL_EMIT_COUNT: usize,
    > Transition<CONDITION_COUNT, KEY_EMIT_COUNT, INTERNAL_EMIT_COUNT>
{
    const fn as_dyn(&self) -> &dyn DynTransition {
        self
    }
}

trait DynTransition: Send + Sync + 'static {
    fn conditions(&self) -> &[TransitionCondition];
    fn key_event_emissions(&self) -> &[KeyEvent];
    fn internal_event_emissions(&self) -> &[InternalEvent];
    fn target(&self) -> &'static dyn DynState;
    fn evaluate(
        &self,
        elapsed: Milliseconds,
        key: Option<InputEvent>,
        state: StateFlags,
    ) -> Option<(&[KeyEvent], &[InternalEvent], &'static dyn DynState)> {
        if self
            .conditions()
            .iter()
            .all(|c| c.evaluate(elapsed, key, state))
        {
            Some((
                self.key_event_emissions(),
                self.internal_event_emissions(),
                self.target(),
            ))
        } else {
            None
        }
    }
}

impl<
        const CONDITION_COUNT: usize,
        const KEY_EMIT_COUNT: usize,
        const INTERNAL_EMIT_COUNT: usize,
    > DynTransition for Transition<CONDITION_COUNT, KEY_EMIT_COUNT, INTERNAL_EMIT_COUNT>
{
    fn conditions(&self) -> &[TransitionCondition] {
        &self.conditions
    }

    fn key_event_emissions(&self) -> &[KeyEvent] {
        &self.key_event_emissions
    }

    fn internal_event_emissions(&self) -> &[InternalEvent] {
        &self.internal_event_emissions
    }

    fn target(&self) -> &'static dyn DynState {
        self.target
    }
}

struct State<const TRANSITION_COUNT: usize> {
    name: &'static str,
    transitions: [&'static dyn DynTransition; TRANSITION_COUNT],
}

impl<const TRANSITION_COUNT: usize> State<TRANSITION_COUNT> {
    const fn as_dyn(&self) -> &dyn DynState {
        self
    }
}

trait DynState: Send + Sync + 'static {
    fn transitions(&self) -> &[&'static dyn DynTransition];
    fn name(&self) -> &str;
}

impl<const SIZE: usize> DynState for State<SIZE> {
    fn transitions(&self) -> &[&'static dyn DynTransition] {
        &self.transitions
    }

    fn name(&self) -> &str {
        self.name
    }
}

impl PartialEq for dyn DynState {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl std::fmt::Debug for &dyn DynState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DynState {{{}}}", self.name())
    }
}

#[cfg(test)]
mod tests {
    struct TickerClock(u32);

    impl TickerClock {
        fn tick(&mut self) {
            self.0 += 1;
        }

        fn tick_n(&mut self, n: u32) {
            self.0 += n;
        }

        fn now(&self) -> Instant<TickerClock> {
            self.try_now().unwrap()
        }
    }

    impl embedded_time::Clock for TickerClock {
        type T = u32;
        const SCALING_FACTOR: embedded_time::rate::Fraction =
            embedded_time::rate::Fraction::new(1, 1_000);

        fn try_now(&self) -> Result<embedded_time::Instant<Self>, embedded_time::clock::Error> {
            Ok(embedded_time::Instant::new(self.0))
        }
    }

    use std::assert_matches::assert_matches;
    use std::sync::atomic::AtomicU32;

    use embedded_time::duration::Milliseconds;
    use embedded_time::Instant;
    use embedded_time::{duration::Extensions, Clock};

    use crate::{
        DynState, DynTransition, GlobalState, InternalEvent, KeyEvent, State, StateFlags,
        Transition, TransitionCondition,
    };

    #[test]
    fn basic() {
        static A: State<1> = State {
            name: "A",
            transitions: [A_0.as_dyn()],
        };

        static A_0: Transition<1, 1, 0> = Transition {
            conditions: [TransitionCondition::pressed_single(0)],
            key_event_emissions: [KeyEvent::Press(0)],
            internal_event_emissions: [],
            target: B.as_dyn(),
        };

        static B: State<1> = State {
            name: "B",
            transitions: [B_0.as_dyn()],
        };

        static B_0: Transition<1, 1, 0> = Transition {
            conditions: [TransitionCondition::depressed_single(0)],
            key_event_emissions: [KeyEvent::Depress(0)],
            internal_event_emissions: [],
            target: A.as_dyn(),
        };

        let clock = TickerClock(0);
        let now = clock.now();

        let mut state = GlobalState {
            flags: StateFlags::empty(),
            entered_state: now,
            current_state: A.as_dyn(),
        };

        for _ in 0..10 {
            let s = state.push(now, crate::InputEvent::Press(0));
            assert_matches!(s, [KeyEvent::Press(0)]);

            let s = state.push(now, crate::InputEvent::Depress(0));
            assert_matches!(s, [KeyEvent::Depress(0)]);
        }
    }

    #[test]
    fn mod_tap_better() {
        static ROOT: State<3> = State {
            name: "ROOT",
            transitions: [ROOT_0.as_dyn(), ROOT_PRESS_1.as_dyn(), ROOT_RESET.as_dyn()],
        };

        static ROOT_0: Transition<1, 0, 0> = Transition {
            conditions: [TransitionCondition::pressed_single(0)],
            key_event_emissions: [],
            internal_event_emissions: [],
            target: MOD.as_dyn(),
        };

        static ROOT_PRESS_1: Transition<1, 1, 0> = Transition {
            conditions: [TransitionCondition::pressed_single(1)],
            key_event_emissions: [KeyEvent::Press(1)],
            internal_event_emissions: [],
            target: PRESS_1.as_dyn(),
        };

        // we'll probably have it so that if a normal key is currently being pressed, you can't enter a mod-tap, instead it will
        // press the tap key of the mod tap
        static PRESS_1: State<1> = State {
            name: "PRESS_1",
            transitions: [PRESS_1_DEPRESS.as_dyn()], //, PRESS_1_OTHER.as_dyn()]
        };

        // static PRESS_1_OTHER: Transition<1, 1, 0> = Transition {
        //     conditions: [
        //         TransitionCondition::pressed_single(3),
        //     ],
        //     key_event_emissions: [KeyEvent::Depress(1), KeyEvent::Press(3)],
        //     internal_event_emissions: [],
        //     target: ROOT.as_dyn(),
        // };

        static PRESS_1_DEPRESS: Transition<1, 1, 0> = Transition {
            conditions: [TransitionCondition::depressed_single(1)],
            key_event_emissions: [KeyEvent::Depress(1)],
            internal_event_emissions: [],
            target: ROOT.as_dyn(),
        };

        static ROOT_RESET: Transition<2, 1, 1> = Transition {
            conditions: [
                TransitionCondition::StateSet(StateFlags::SHFT),
                TransitionCondition::depressed_single(0),
            ],
            key_event_emissions: [KeyEvent::Depress(2)],
            internal_event_emissions: [InternalEvent::UnsetGlobalState(StateFlags::SHFT)],
            target: ROOT.as_dyn(),
        };

        static MOD: State<3> = State {
            name: "MOD",
            transitions: [
                MOD_TAP_TRANS.as_dyn(),
                MOD_TAP_OTHER_TRANS.as_dyn(),
                MOD_HOLD_TRANS.as_dyn(),
            ],
        };

        static MOD_TAP_TRANS: Transition<2, 2, 0> = Transition {
            conditions: [
                TransitionCondition::depressed_single(0),
                TransitionCondition::ElapsedLess(Milliseconds(5_u32)),
            ],
            key_event_emissions: [KeyEvent::Press(0), KeyEvent::Depress(0)],
            internal_event_emissions: [],
            target: ROOT.as_dyn(),
        };

        static MOD_TAP_OTHER_TRANS: Transition<1, 2, 1> = Transition {
            conditions: [TransitionCondition::pressed_single(1)],
            key_event_emissions: [KeyEvent::Press(2), KeyEvent::Press(1)],
            internal_event_emissions: [InternalEvent::SetGlobalState(StateFlags::SHFT)],
            target: PRESS_1.as_dyn(),
        };

        static MOD_HOLD_TRANS: Transition<1, 1, 1> = Transition {
            conditions: [TransitionCondition::ElapsedGreater(Milliseconds(5_u32))],
            key_event_emissions: [KeyEvent::Press(2)],
            internal_event_emissions: [InternalEvent::SetGlobalState(StateFlags::SHFT)],
            target: ROOT.as_dyn(),
        };

        let mut clock = TickerClock(0);

        let mut state = GlobalState {
            flags: StateFlags::empty(),
            entered_state: clock.now(),
            current_state: ROOT.as_dyn(),
        };

        for _ in 0..10 {
            assert_eq!(state.flags, StateFlags::empty());
            assert_eq!(state.current_state, ROOT.as_dyn());

            let s = state.push(clock.now(), crate::InputEvent::Press(0));
            assert_eq!(state.current_state, MOD.as_dyn());
            assert_matches!(s, []);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(0));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_eq!(state.flags, StateFlags::empty());
            assert_matches!(s, [KeyEvent::Press(0), KeyEvent::Depress(0)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(0));
            assert_eq!(state.current_state, MOD.as_dyn());
            assert_matches!(s, []);

            clock.tick_n(8);

            let s = state.tick(clock.now());
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Press(2)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(1));
            assert_eq!(state.current_state, PRESS_1.as_dyn());
            assert_matches!(s, [KeyEvent::Press(1)]);

            clock.tick();
            let s = state.push(clock.now(), crate::InputEvent::Depress(1));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(1)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(0));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(2)]);
            assert_eq!(state.flags, StateFlags::empty());

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(0));
            assert_eq!(state.current_state, MOD.as_dyn());
            assert_matches!(s, []);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(1));
            assert_eq!(state.current_state, PRESS_1.as_dyn());
            assert_matches!(s, [KeyEvent::Press(2), KeyEvent::Press(1)]);

            clock.tick();
            let s = state.push(clock.now(), crate::InputEvent::Depress(1));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(1)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(1));
            assert_eq!(state.current_state, PRESS_1.as_dyn());
            assert_matches!(s, [KeyEvent::Press(1)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(1));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(1)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(0));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(2)]);
            assert_eq!(state.flags, StateFlags::empty());

            clock.tick()
        }
    }

    #[test]
    fn mod_tap() {
        static ROOT: State<1> = State {
            name: "ROOT",
            transitions: [ROOT_0.as_dyn()],
        };

        static ROOT_0: Transition<1, 0, 0> = Transition {
            conditions: [TransitionCondition::pressed_single(0)],
            key_event_emissions: [],
            internal_event_emissions: [],
            target: MOD.as_dyn(),
        };

        static MOD: State<3> = State {
            name: "MOD",
            transitions: [
                MOD_TAP_TRANS.as_dyn(),
                MOD_TAP_OTHER_TRANS.as_dyn(),
                MOD_HOLD_TRANS.as_dyn(),
            ],
        };

        static MOD_TAP_TRANS: Transition<2, 2, 0> = Transition {
            conditions: [
                TransitionCondition::depressed_single(0),
                TransitionCondition::ElapsedLess(Milliseconds(5_u32)),
            ],
            key_event_emissions: [KeyEvent::Press(0), KeyEvent::Depress(0)],
            internal_event_emissions: [],
            target: ROOT.as_dyn(),
        };

        static MOD_TAP_OTHER_TRANS: Transition<1, 3, 1> = Transition {
            conditions: [TransitionCondition::pressed_single(1)],
            key_event_emissions: [KeyEvent::Press(2), KeyEvent::Press(1), KeyEvent::Depress(1)],
            internal_event_emissions: [InternalEvent::SetGlobalState(StateFlags::SHFT)],
            target: MOD_HOLD.as_dyn(),
        };

        static MOD_HOLD_TRANS: Transition<1, 1, 1> = Transition {
            conditions: [TransitionCondition::ElapsedGreater(Milliseconds(5_u32))],
            key_event_emissions: [KeyEvent::Press(2)],
            internal_event_emissions: [InternalEvent::SetGlobalState(StateFlags::SHFT)],
            target: MOD_HOLD.as_dyn(),
        };

        static MOD_HOLD: State<2> = State {
            name: "MOD_HOLD",
            transitions: [
                MOD_HOLD_DEPRESS_TRANS.as_dyn(),
                MOD_HOLD_OTHER_TRANS.as_dyn(),
            ],
        };

        static MOD_HOLD_DEPRESS_TRANS: Transition<1, 1, 1> = Transition {
            conditions: [TransitionCondition::depressed_single(0)],
            key_event_emissions: [KeyEvent::Depress(2)],
            internal_event_emissions: [InternalEvent::UnsetGlobalState(StateFlags::SHFT)],
            target: ROOT.as_dyn(),
        };

        static MOD_HOLD_OTHER_TRANS: Transition<1, 2, 0> = Transition {
            conditions: [TransitionCondition::pressed_single(1)],
            key_event_emissions: [KeyEvent::Press(1), KeyEvent::Depress(1)],
            internal_event_emissions: [],
            target: MOD_HOLD.as_dyn(),
        };

        let mut clock = TickerClock(0);

        let mut state = GlobalState {
            flags: StateFlags::empty(),
            entered_state: clock.now(),
            current_state: ROOT.as_dyn(),
        };

        for _ in 0..10 {
            let s = state.push(clock.now(), crate::InputEvent::Press(0));
            assert_eq!(state.current_state, MOD.as_dyn());
            assert_matches!(s, []);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(0));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Press(0), KeyEvent::Depress(0)]);

            let s = state.push(clock.now(), crate::InputEvent::Press(0));
            assert_eq!(state.current_state, MOD.as_dyn());
            assert_matches!(s, []);

            clock.tick_n(8);

            let s = state.tick(clock.now());
            assert_eq!(state.current_state, MOD_HOLD.as_dyn());
            assert_matches!(s, [KeyEvent::Press(2)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(1));
            assert_eq!(state.current_state, MOD_HOLD.as_dyn());
            assert_matches!(s, [KeyEvent::Press(1), KeyEvent::Depress(1)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(0));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(2)]);
            assert_eq!(state.flags, StateFlags::empty());

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(0));
            assert_eq!(state.current_state, MOD.as_dyn());
            assert_matches!(s, []);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(1));
            assert_eq!(state.current_state, MOD_HOLD.as_dyn());
            assert_matches!(
                s,
                [KeyEvent::Press(2), KeyEvent::Press(1), KeyEvent::Depress(1)]
            );

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Press(1));
            assert_eq!(state.current_state, MOD_HOLD.as_dyn());
            assert_matches!(s, [KeyEvent::Press(1), KeyEvent::Depress(1)]);

            clock.tick();

            let s = state.push(clock.now(), crate::InputEvent::Depress(0));
            assert_eq!(state.current_state, ROOT.as_dyn());
            assert_matches!(s, [KeyEvent::Depress(2)]);
            assert_eq!(state.flags, StateFlags::empty());
        }
    }
}
