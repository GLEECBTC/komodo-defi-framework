use crate::prelude::*;
use crate::state_machine::{ChangeGuard, ErrorGuard};
use async_trait::async_trait;

/// A trait representing the initial state of a state machine.
pub trait InitialState {
    /// The type of state machine associated with this initial state.
    type StateMachine: StorableStateMachine;
}

/// A trait for handling new states in a state machine.
#[async_trait]
pub trait OnNewState<S>: StateMachineTrait {
    /// Handles a new state.
    ///
    /// # Parameters
    ///
    /// - `state`: A reference to the new state to be handled.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(<Self as StateMachineTrait>::Error)`).
    async fn on_new_state(&mut self, state: &S) -> Result<(), <Self as StateMachineTrait>::Error>;
}

pub trait StateMachineDbRepr: Send {
    type Event: Send;

    fn add_event(&mut self, event: Self::Event);
}

/// A trait for the storage of state machine events.
#[async_trait]
pub trait StateMachineStorage: Send + Sync {
    /// The type representing a unique identifier for a state machine.
    type MachineId: Send + Sync;
    /// The type representing state machine's DB representation.
    type DbRepr: StateMachineDbRepr;
    /// The type representing an error that can occur during storage operations.
    type Error: Send;

    /// Stores a DB representation of a given state machine.
    ///
    /// # Parameters
    ///
    /// - `id`: The unique identifier of the state machine.
    /// - `repr`: The representation to be stored.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(Self::Error)`).
    async fn store_repr(&mut self, id: Self::MachineId, repr: Self::DbRepr) -> Result<(), Self::Error>;

    /// Gets a DB representation of a given state machine.
    ///
    /// # Parameters
    ///
    /// - `id`: The unique identifier of the state machine.
    ///
    /// # Returns
    ///
    /// A `Result` containing representation (`Ok(Self::DbRepr)`) or an error (`Err(Self::Error)`).
    async fn get_repr(&self, id: Self::MachineId) -> Result<Self::DbRepr, Self::Error>;

    /// Returns whether DB stores a state machine with the given id.
    ///
    /// # Parameters
    ///
    /// - `id`: The unique identifier of the state machine.
    ///
    /// # Returns
    ///
    /// A `Result` indicating the existense of the DB record.
    async fn has_record_for(&mut self, id: &Self::MachineId) -> Result<bool, Self::Error>;

    /// Stores an event for a given state machine.
    ///
    /// # Parameters
    ///
    /// - `id`: The unique identifier of the state machine.
    /// - `event`: The event to be stored.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(Self::Error)`).
    async fn store_event(
        &mut self,
        id: Self::MachineId,
        event: <Self::DbRepr as StateMachineDbRepr>::Event,
    ) -> Result<(), Self::Error>;

    /// Retrieves a list of unfinished state machines.
    ///
    /// # Returns
    ///
    /// A `Result` containing a vector of machine IDs or an error (`Err(Self::Error)`).
    async fn get_unfinished(&self) -> Result<Vec<Self::MachineId>, Self::Error>;

    /// Marks a state machine as finished.
    ///
    /// # Parameters
    ///
    /// - `id`: The unique identifier of the state machine to be marked as finished.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(Self::Error)`).
    async fn mark_finished(&mut self, id: Self::MachineId) -> Result<(), Self::Error>;
}

pub trait RestoredState: StorableState + Send {
    fn into_state(self: Box<Self>) -> Box<dyn State<StateMachine = Self::StateMachine>>;
}

impl<T: StorableState + State<StateMachine = Self::StateMachine> + Send> RestoredState for T {
    fn into_state(self: Box<Self>) -> Box<dyn State<StateMachine = Self::StateMachine>> {
        self
    }
}

/// A struct representing a restored state machine.
pub struct RestoredMachine<M: StorableStateMachine> {
    machine: M,
}

impl<M: StorableStateMachine> RestoredMachine<M> {
    pub fn new(machine: M) -> Self {
        RestoredMachine { machine }
    }

    pub async fn kickstart(
        &mut self,
        from_state: Box<dyn RestoredState<StateMachine = M>>,
    ) -> Result<M::Result, M::Error> {
        let event = from_state.get_event();
        self.machine.on_kickstart_event(event);
        self.machine.run(from_state.into_state()).await
    }
}

/// A trait for storable state machines.
#[async_trait]
pub trait StorableStateMachine: Send + Sync + Sized + 'static {
    /// The type of storage for the state machine.
    type Storage: StateMachineStorage;
    /// The result type of the state machine.
    type Result: Send;
    /// The error type of the state machine
    type Error: From<<Self::Storage as StateMachineStorage>::Error> + Send;
    /// The reentrancy lock type of the state machine
    type ReentrancyLock: Send;
    /// The additional context required to recreate state machine.
    type RecreateCtx: Send;
    /// Type representing the error, which can happen during state machine's re-creation
    type RecreateError: Send;

    /// Returns State machine's DB representation()
    fn to_db_repr(&self) -> <Self::Storage as StateMachineStorage>::DbRepr;

    /// Gets a mutable reference to the storage for the state machine.
    fn storage(&mut self) -> &mut Self::Storage;

    /// Gets the unique identifier of the state machine.
    fn id(&self) -> <Self::Storage as StateMachineStorage>::MachineId;

    /// Recreates a state machine from DB representation.
    ///
    /// # Parameters
    ///
    /// - `id`: The unique identifier of the state machine to be recreated.
    /// - `storage`: Storage instance.
    /// - `repr`: State machine's DB representation.
    /// - `from_repr_ctx`: Additional context required to recreate the state machine.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `RestoredMachine` or an error.
    async fn recreate_machine(
        id: <Self::Storage as StateMachineStorage>::MachineId,
        storage: Self::Storage,
        repr: <Self::Storage as StateMachineStorage>::DbRepr,
        from_repr_ctx: Self::RecreateCtx,
    ) -> Result<(RestoredMachine<Self>, Box<dyn RestoredState<StateMachine = Self>>), Self::RecreateError>;

    /// Stores an event for the state machine.
    ///
    /// # Parameters
    ///
    /// - `event`: The event to be stored.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(Self::Error)`).
    async fn store_event(
        &mut self,
        event: <<Self::Storage as StateMachineStorage>::DbRepr as StateMachineDbRepr>::Event,
    ) -> Result<(), <Self::Storage as StateMachineStorage>::Error> {
        let id = self.id();
        self.storage().store_event(id, event).await
    }

    /// Marks the state machine as finished.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(Self::Error)`).
    async fn mark_finished(&mut self) -> Result<(), <Self::Storage as StateMachineStorage>::Error> {
        let id = self.id();
        self.storage().mark_finished(id).await
    }

    /// Attempts to acquire reentrancy lock for a state machine
    ///
    /// # Returns
    ///
    /// A `Result` containing reentrancy lock guard success (`Ok(Self::ReentrancyLock)`) or an error (`Err(Self::Error)`).
    async fn acquire_reentrancy_lock(&self) -> Result<Self::ReentrancyLock, Self::Error>;

    /// Spawns the thread or future renewing the reentrancy lock.
    /// Graceful shutdown of this activity is responsibility of an actual implementation.
    fn spawn_reentrancy_lock_renew(&mut self, guard: Self::ReentrancyLock);

    /// Initializes additional context actions (spawn futures, etc.)
    fn init_additional_context(&mut self);

    /// Cleans additional context up
    fn clean_up_context(&mut self);

    /// Perform additional actions when specific state's event is triggered (notify context, etc.)
    fn on_event(&mut self, event: &<<Self::Storage as StateMachineStorage>::DbRepr as StateMachineDbRepr>::Event);

    /// Perform additional actions using event received on kick-started state
    fn on_kickstart_event(
        &mut self,
        event: <<Self::Storage as StateMachineStorage>::DbRepr as StateMachineDbRepr>::Event,
    );
}

// Ensure that StandardStateMachine won't be occasionally implemented for StorableStateMachine.
// Users of StorableStateMachine must be prevented from using ChangeStateExt::change_state
// because it doesn't call machine.on_new_state.
impl<T: StorableStateMachine> !StandardStateMachine for T {}

// Prevent implementing both StorableState and InitialState at the same time
impl<T: StorableState> !InitialState for T {}

#[async_trait]
impl<T: StorableStateMachine> StateMachineTrait for T {
    type Result = T::Result;
    type Error = T::Error;

    async fn on_start(&mut self) -> Result<(), Self::Error> {
        let reentrancy_lock = self.acquire_reentrancy_lock().await?;
        let id = self.id();
        if !self.storage().has_record_for(&id).await? {
            let repr = self.to_db_repr();
            self.storage().store_repr(id, repr).await?;
        }
        self.spawn_reentrancy_lock_renew(reentrancy_lock);
        self.init_additional_context();
        Ok(())
    }

    async fn on_finished(&mut self) -> Result<(), T::Error> {
        self.mark_finished().await?;
        self.clean_up_context();
        Ok(())
    }
}

/// A trait for storable states.
pub trait StorableState {
    /// The type of state machine associated with this state.
    type StateMachine: StorableStateMachine;

    /// Gets the event associated with this state.
    fn get_event(&self) -> <<<Self::StateMachine as StorableStateMachine>::Storage as StateMachineStorage>::DbRepr as StateMachineDbRepr>::Event;
}

/// Implementation of `OnNewState` for storable state machines and their related states.
#[async_trait]
impl<T: StorableStateMachine + Sync, S: StorableState<StateMachine = T> + Sync> OnNewState<S> for T {
    /// Handles a new state.
    ///
    /// # Parameters
    ///
    /// - `state`: A reference to the new state to be handled.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success (`Ok(())`) or an error (`Err(Self::Error)`).
    async fn on_new_state(&mut self, state: &S) -> Result<(), T::Error> {
        let event = state.get_event();
        self.on_event(&event);
        Ok(self.store_event(event).await?)
    }
}

/// An asynchronous function for changing the state of a storable state machine.
///
/// # Parameters
///
/// - `next_state`: The next state to transition to.
/// - `machine`: A mutable reference to the state machine.
///
/// # Returns
///
/// A `StateResult` indicating success or an error.
///
/// # Generic Parameters
///
/// - `Next`: The type of the next state.
async fn change_state_impl<Next>(next_state: Next, machine: &mut Next::StateMachine) -> StateResult<Next::StateMachine>
where
    Next: State + ChangeStateOnNewExt,
    Next::StateMachine: OnNewState<Next> + Sync,
{
    if let Err(e) = machine.on_new_state(&next_state).await {
        return StateResult::Error(ErrorGuard::new(e));
    }
    StateResult::ChangeState(ChangeGuard::next(next_state))
}

/// A trait for state transition functionality.
#[async_trait]
pub trait ChangeStateOnNewExt {
    /// Change the state to the `next_state`.
    ///
    /// # Parameters
    ///
    /// - `next_state`: The next state to transition to.
    /// - `machine`: A mutable reference to the state machine.
    ///
    /// # Returns
    ///
    /// A `StateResult` indicating success or an error.
    ///
    /// # Generic Parameters
    ///
    /// - `Next`: The type of the next state.
    async fn change_state<Next>(next_state: Next, machine: &mut Next::StateMachine) -> StateResult<Next::StateMachine>
    where
        Self: Sized,
        Next: State + TransitionFrom<Self> + ChangeStateOnNewExt,
        Next::StateMachine: OnNewState<Next> + Sync,
    {
        change_state_impl(next_state, machine).await
    }
}

impl<M: StorableStateMachine, T: StorableState<StateMachine = M>> ChangeStateOnNewExt for T {}

/// A trait for initial state change functionality.
#[async_trait]
pub trait ChangeInitialStateExt: InitialState {
    /// Change the state to the `next_state`.
    ///
    /// # Parameters
    ///
    /// - `next_state`: The next state to transition to.
    /// - `machine`: A mutable reference to the state machine.
    ///
    /// # Returns
    ///
    /// A `StateResult` indicating success or an error.
    ///
    /// # Generic Parameters
    ///
    /// - `Next`: The type of the next state.
    async fn change_state<Next>(next_state: Next, machine: &mut Next::StateMachine) -> StateResult<Next::StateMachine>
    where
        Self: Sized,
        Next: State + TransitionFrom<Self> + ChangeStateOnNewExt,
        Next::StateMachine: OnNewState<Next> + Sync,
    {
        change_state_impl(next_state, machine).await
    }
}

impl<M: StorableStateMachine, T: InitialState<StateMachine = M>> ChangeInitialStateExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use common::block_on;
    use std::collections::HashMap;
    use std::convert::Infallible;

    struct StorageTest {
        events_unfinished: HashMap<usize, Vec<TestEvent>>,
        events_finished: HashMap<usize, Vec<TestEvent>>,
    }

    impl StorageTest {
        fn empty() -> Self {
            StorageTest {
                events_unfinished: HashMap::new(),
                events_finished: HashMap::new(),
            }
        }
    }

    struct StorableStateMachineTest {
        id: usize,
        storage: StorageTest,
    }

    #[derive(Debug, Eq, PartialEq)]
    enum TestEvent {
        ForState2,
        ForState3,
        ForState4,
    }

    struct TestStateMachineRepr {}

    impl StateMachineDbRepr for TestStateMachineRepr {
        type Event = TestEvent;

        fn add_event(&mut self, _event: Self::Event) {
            unimplemented!()
        }
    }

    #[async_trait]
    impl StateMachineStorage for StorageTest {
        type MachineId = usize;
        type DbRepr = TestStateMachineRepr;
        type Error = Infallible;

        async fn store_repr(&mut self, _id: Self::MachineId, _repr: Self::DbRepr) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn get_repr(&self, _id: Self::MachineId) -> Result<Self::DbRepr, Self::Error> {
            Ok(TestStateMachineRepr {})
        }

        async fn has_record_for(&mut self, _id: &Self::MachineId) -> Result<bool, Self::Error> {
            Ok(false)
        }

        async fn store_event(&mut self, machine_id: usize, event: TestEvent) -> Result<(), Self::Error> {
            self.events_unfinished.entry(machine_id).or_default().push(event);
            Ok(())
        }

        async fn get_unfinished(&self) -> Result<Vec<Self::MachineId>, Self::Error> {
            Ok(self.events_unfinished.keys().copied().collect())
        }

        async fn mark_finished(&mut self, id: Self::MachineId) -> Result<(), Self::Error> {
            let events = self.events_unfinished.remove(&id).unwrap();
            self.events_finished.insert(id, events);
            Ok(())
        }
    }

    #[async_trait]
    impl StorableStateMachine for StorableStateMachineTest {
        type Storage = StorageTest;
        type Result = ();
        type Error = Infallible;
        type ReentrancyLock = ();
        type RecreateCtx = ();
        type RecreateError = Infallible;

        fn to_db_repr(&self) -> TestStateMachineRepr {
            TestStateMachineRepr {}
        }

        fn storage(&mut self) -> &mut Self::Storage {
            &mut self.storage
        }

        fn id(&self) -> <Self::Storage as StateMachineStorage>::MachineId {
            self.id
        }

        async fn recreate_machine(
            id: <Self::Storage as StateMachineStorage>::MachineId,
            storage: Self::Storage,
            _repr: <Self::Storage as StateMachineStorage>::DbRepr,
            _recreate_ctx: Self::RecreateCtx,
        ) -> Result<(RestoredMachine<Self>, Box<dyn RestoredState<StateMachine = Self>>), Self::RecreateError> {
            let events = storage.events_unfinished.get(&id).unwrap();
            let current_state: Box<dyn RestoredState<StateMachine = Self>> = match events.last() {
                Some(TestEvent::ForState2) => Box::new(State2 {}),
                _ => unimplemented!(),
            };
            let machine = StorableStateMachineTest { id, storage };
            Ok((RestoredMachine { machine }, current_state))
        }

        async fn acquire_reentrancy_lock(&self) -> Result<Self::ReentrancyLock, Self::Error> {
            Ok(())
        }

        fn spawn_reentrancy_lock_renew(&mut self, _guard: Self::ReentrancyLock) {}

        fn init_additional_context(&mut self) {}

        fn clean_up_context(&mut self) {}

        fn on_event(&mut self, _event: &<<Self::Storage as StateMachineStorage>::DbRepr as StateMachineDbRepr>::Event) {
        }

        fn on_kickstart_event(
            &mut self,
            _event: <<Self::Storage as StateMachineStorage>::DbRepr as StateMachineDbRepr>::Event,
        ) {
        }
    }

    struct State1 {}

    impl InitialState for State1 {
        type StateMachine = StorableStateMachineTest;
    }

    struct State2 {}

    impl StorableState for State2 {
        type StateMachine = StorableStateMachineTest;

        fn get_event(&self) -> TestEvent {
            TestEvent::ForState2
        }
    }

    impl TransitionFrom<State1> for State2 {}

    struct State3 {}

    impl StorableState for State3 {
        type StateMachine = StorableStateMachineTest;

        fn get_event(&self) -> TestEvent {
            TestEvent::ForState3
        }
    }

    impl TransitionFrom<State2> for State3 {}

    struct State4 {}

    impl StorableState for State4 {
        type StateMachine = StorableStateMachineTest;

        fn get_event(&self) -> TestEvent {
            TestEvent::ForState4
        }
    }

    impl TransitionFrom<State3> for State4 {}

    #[async_trait]
    impl LastState for State4 {
        type StateMachine = StorableStateMachineTest;

        async fn on_changed(self: Box<Self>, _ctx: &mut Self::StateMachine) -> () {}
    }

    #[async_trait]
    impl State for State1 {
        type StateMachine = StorableStateMachineTest;

        async fn on_changed(self: Box<Self>, ctx: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
            Self::change_state(State2 {}, ctx).await
        }
    }

    #[async_trait]
    impl State for State2 {
        type StateMachine = StorableStateMachineTest;

        async fn on_changed(self: Box<Self>, ctx: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
            Self::change_state(State3 {}, ctx).await
        }
    }

    #[async_trait]
    impl State for State3 {
        type StateMachine = StorableStateMachineTest;

        async fn on_changed(self: Box<Self>, ctx: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
            Self::change_state(State4 {}, ctx).await
        }
    }

    #[test]
    fn run_storable_state_machine() {
        let mut machine = StorableStateMachineTest {
            id: 1,
            storage: StorageTest::empty(),
        };
        block_on(machine.run(Box::new(State1 {}))).unwrap();

        let expected_events = HashMap::from_iter([(
            1,
            vec![TestEvent::ForState2, TestEvent::ForState3, TestEvent::ForState4],
        )]);
        assert_eq!(expected_events, machine.storage.events_finished);
    }

    #[test]
    fn restore_state_machine() {
        let mut storage = StorageTest::empty();
        let id = 1;
        storage.events_unfinished.insert(1, vec![TestEvent::ForState2]);
        let (mut restored_machine, from_state) = block_on(StorableStateMachineTest::recreate_machine(
            id,
            storage,
            TestStateMachineRepr {},
            (),
        ))
        .unwrap();

        block_on(restored_machine.kickstart(from_state)).unwrap();

        let expected_events = HashMap::from_iter([(
            1,
            vec![TestEvent::ForState2, TestEvent::ForState3, TestEvent::ForState4],
        )]);
        assert_eq!(expected_events, restored_machine.machine.storage.events_finished);
    }
}
