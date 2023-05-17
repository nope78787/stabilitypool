use std::fmt;

use async_trait::async_trait;
use fedimint_client::sm::DynState;
use fedimint_client::DynGlobalClientContext;
use fedimint_core::core::IntoDynInstance;
use fedimint_core::core::{Decoder, ModuleInstanceId, ModuleKind};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::{CommonModuleGen, ModuleCommon, ModuleConsensusVersion};
use serde::{Deserialize, Serialize};

pub use crate::account::*;
pub use crate::action::{Action, ActionProposed, ActionStaged, ProviderBid, SeekerAction};
pub use crate::epoch::{EpochEnd, EpochOutcome, EpochState};
pub use crate::price::*;

pub mod account;
pub mod action;
pub mod config;
pub mod db;
pub mod epoch;
pub mod price;
pub mod stability_core;

pub const KIND: ModuleKind = ModuleKind::from_static_str("stabilitypool");

#[derive(Debug, Clone)]
pub struct PoolCommonGen;

#[async_trait]
impl CommonModuleGen for PoolCommonGen {
    const CONSENSUS_VERSION: ModuleConsensusVersion = ModuleConsensusVersion(0);
    const KIND: ModuleKind = KIND;

    fn decoder() -> Decoder {
        PoolModuleTypes::decoder()
    }
}

pub type PoolInput = AccountWithdrawal;
pub type PoolOutput = AccountDeposit;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct PoolOutputOutcome(pub secp256k1_zkp::XOnlyPublicKey);

impl fmt::Display for PoolOutputOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PoolOutputOutcome")
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encodable, Decodable)]
pub enum PoolConsensusItem {
    ActionProposed(ActionProposed),
    EpochEnd(EpochEnd),
}

impl fmt::Display for PoolConsensusItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActionProposed(action_proposed) => write!(
                f,
                "[action_proposed] by account:{} for pool_epoch:{}",
                action_proposed.account_id(),
                action_proposed.epoch_id(),
            ),
            Self::EpochEnd(end) => write!(
                f,
                "[epoch_end] epoch_id:{} with price:{:?}",
                end.epoch_id, end.price
            ),
        }
    }
}

impl From<ActionProposed> for PoolConsensusItem {
    fn from(value: ActionProposed) -> Self {
        Self::ActionProposed(value)
    }
}

impl From<EpochEnd> for PoolConsensusItem {
    fn from(value: EpochEnd) -> Self {
        Self::EpochEnd(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsensusItemOutcome {
    Applied,
    Ignored(String),
    Banned(String),
}

use fedimint_core::plugin_types_trait_impl_common;

// #[derive(Debug, Default, Clone)]
// pub struct PoolDecoder;

pub struct PoolModuleTypes;

plugin_types_trait_impl_common!(
    PoolModuleTypes,
    PoolInput,
    PoolOutput,
    PoolOutputOutcome,
    PoolConsensusItem
);

/// Tracks a transaction
#[derive(Debug, Clone, Eq, PartialEq, Decodable, Encodable)]
pub enum PoolStateMachine {
    // Input(Amount, TransactionId, OperationId),
    // Output(Amount, TransactionId, OperationId),
    // Done,
}

/// Data needed by the state machine
#[derive(Debug, Clone)]
pub struct PoolClientContext;

// TODO: Boiler-plate
impl fedimint_client::sm::Context for PoolClientContext {}

impl fedimint_client::sm::State for PoolStateMachine {
    type ModuleContext = PoolClientContext;
    type GlobalContext = DynGlobalClientContext;

    fn transitions(
        &self,
        _context: &Self::ModuleContext,
        _global_context: &Self::GlobalContext,
    ) -> Vec<fedimint_client::sm::StateTransition<Self>> {
        vec![]
    }

    fn operation_id(&self) -> fedimint_client::sm::OperationId {
        fedimint_client::sm::OperationId([0; 32])
    }
}

// TODO: Boiler-plate
impl IntoDynInstance for PoolStateMachine {
    type DynType = DynState<DynGlobalClientContext>;

    fn into_dyn(self, instance_id: ModuleInstanceId) -> Self::DynType {
        DynState::from_typed(instance_id, self)
    }
}
