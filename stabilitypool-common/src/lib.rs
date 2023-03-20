use std::fmt;

use async_trait::async_trait;
use common::PoolModuleTypes;
use config::PoolConfigClient;
use fedimint_core::core::{Decoder, ModuleInstanceId, ModuleKind};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::{CommonModuleGen, ModuleCommon};
use serde::{Deserialize, Serialize};

pub use crate::account::*;
pub use crate::action::*;
pub use crate::epoch::*;
pub use crate::price::*;

pub mod account;
pub mod action;
pub mod common;
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
    const KIND: ModuleKind = KIND;

    fn decoder() -> Decoder {
        PoolModuleTypes::decoder()
    }

    fn hash_client_module(
        config: serde_json::Value,
    ) -> anyhow::Result<bitcoin::hashes::sha256::Hash> {
        serde_json::from_value::<PoolConfigClient>(config)?.consensus_hash()
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
