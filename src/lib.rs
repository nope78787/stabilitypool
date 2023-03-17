use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use common::StabilityPoolModuleTypes;
use fedimint_core::core::{ModuleInstanceId, ModuleKind};
use fedimint_core::db::ModuleDatabaseTransaction;
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::audit::Audit;
use fedimint_core::module::interconnect::ModuleInterconect;
use fedimint_core::module::{
    ApiEndpoint, ApiVersion, ConsensusProposal, InputMeta, IntoModuleError, ModuleConsensusVersion,
    ModuleError, TransactionItemAmount,
};
use fedimint_core::{OutPoint, PeerId, ServerModule};
use serde::{Deserialize, Serialize};

pub use crate::account::*;
pub use crate::action::*;
use crate::config::{EpochConfig, PoolConfig};
pub use crate::config_gen::*;
use crate::db::AccountBalanceKeyPrefix;
pub use crate::epoch::*;
pub use crate::price::*;

mod account;
mod action;
pub mod api;
pub mod common;
pub mod config;
mod config_gen;
pub mod db;
mod epoch;
mod price;
pub mod stability_core;

pub const KIND: ModuleKind = ModuleKind::from_static_str("stabilitypool");

#[derive(Debug)]
pub struct StabilityPool {
    pub cfg: PoolConfig,
    pub oracle: Box<dyn OracleClient>,
    pub backoff: BackOff,
    pub proposed_db: ActionProposedDb,
}

#[derive(Debug, Clone)]
pub struct PoolVerificationCache;

impl fedimint_core::server::VerificationCache for PoolVerificationCache {}

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

impl StabilityPool {
    fn epoch_config(&self) -> &EpochConfig {
        &self.cfg.consensus.epoch
    }

    fn oracle(&self) -> &dyn OracleClient {
        &*self.oracle
    }
}

#[async_trait]
impl ServerModule for StabilityPool {
    type Gen = config_gen::PoolConfigGenerator;
    type Common = StabilityPoolModuleTypes;
    type VerificationCache = PoolVerificationCache;

    fn versions(&self) -> (ModuleConsensusVersion, &[ApiVersion]) {
        (
            ModuleConsensusVersion(1),
            &[ApiVersion { major: 1, minor: 1 }],
        )
    }

    async fn await_consensus_proposal(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
    ) {
        // This method is `select_all`ed on across all modules.
        // We block until at least one of these happens:
        // * At least one proposed action is avaliable
        // * Duration past requires us to send `PoolConsensusItem::EpochEnd`
        loop {
            if action::can_propose(dbtx, &self.proposed_db).await {
                tracing::debug!("can propose: action");
                return;
            }
            if epoch::can_propose(dbtx, &self.backoff, self.epoch_config()).await {
                tracing::debug!("can propose: epoch");
                return;
            }

            #[cfg(not(target_family = "wasm"))]
            fedimint_core::task::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn consensus_proposal(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
    ) -> ConsensusProposal<PoolConsensusItem> {
        let mut items = Vec::new();

        items.append(
            &mut epoch::consensus_proposal(dbtx, &self.backoff, self.epoch_config(), self.oracle())
                .await,
        );
        items.append(&mut action::consensus_proposal(dbtx, &self.proposed_db).await);
        ConsensusProposal::Contribute(items)
    }

    async fn begin_consensus_epoch<'a, 'b>(
        &'a self,
        dbtx: &mut ModuleDatabaseTransaction<'b, ModuleInstanceId>,
        consensus_items: Vec<(PeerId, PoolConsensusItem)>,
    ) {
        for (peer_id, item) in consensus_items {
            let outcome = match item {
                PoolConsensusItem::ActionProposed(action_proposed) => {
                    action::process_consensus_item(dbtx, &self.proposed_db, action_proposed).await
                }
                PoolConsensusItem::EpochEnd(epoch_end) => {
                    epoch::process_consensus_item(dbtx, self.epoch_config(), peer_id, epoch_end)
                        .await
                }
            };

            match outcome {
                ConsensusItemOutcome::Applied => {
                    tracing::info!(peer = peer_id.to_usize(), "APPLIED")
                }
                ConsensusItemOutcome::Ignored(reason) => {
                    tracing::debug!(peer = peer_id.to_usize(), reason, "IGNORED")
                }
                ConsensusItemOutcome::Banned(reason) => {
                    tracing::warn!(peer = peer_id.to_usize(), reason, "BANNED")
                }
            }
        }
    }

    fn build_verification_cache<'a>(
        &'a self,
        _inputs: impl Iterator<Item = &'a PoolInput> + Send,
    ) -> Self::VerificationCache {
        PoolVerificationCache
    }

    async fn validate_input<'a, 'b>(
        &self,
        _interconnect: &dyn ModuleInterconect,
        dbtx: &mut ModuleDatabaseTransaction<'b, ModuleInstanceId>,
        _verification_cache: &Self::VerificationCache,
        withdrawal: &'a PoolInput,
    ) -> Result<InputMeta, ModuleError> {
        let avaliable = dbtx
            .get_value(&db::AccountBalanceKey(withdrawal.account))
            .await
            .map(|acc| acc.unlocked)
            .unwrap_or(fedimint_core::Amount::ZERO);

        // TODO: we should also deduct seeker/provider actions that are set for the next
        // round

        if avaliable < withdrawal.amount {
            return Err(WithdrawalError::UnavaliableFunds {
                amount: withdrawal.amount,
                avaliable,
            })
            .into_module_error_other();
        }

        Ok(InputMeta {
            amount: TransactionItemAmount {
                amount: withdrawal.amount,
                // TODO: Figure out how to do fees later.
                fee: fedimint_core::Amount::ZERO,
            },
            puk_keys: [withdrawal.account].into(),
        })
    }

    async fn apply_input<'a, 'b, 'c>(
        &'a self,
        interconnect: &'a dyn ModuleInterconect,
        dbtx: &mut ModuleDatabaseTransaction<'c, ModuleInstanceId>,
        withdrawal: &'b PoolInput,
        verification_cache: &Self::VerificationCache,
    ) -> Result<InputMeta, ModuleError> {
        let meta = self
            .validate_input(interconnect, dbtx, verification_cache, withdrawal)
            .await?;

        tracing::debug!(account = %withdrawal.account, amount = %meta.amount.amount, "Stability pool withdrawal");

        let mut account = dbtx
            .get_value(&db::AccountBalanceKey(withdrawal.account))
            .await
            .unwrap_or_default();

        account.unlocked.msats = account
            .unlocked
            .msats
            .checked_sub(withdrawal.amount.msats)
            .expect("withdrawal amount should already be checked");

        dbtx.insert_entry(&db::AccountBalanceKey(withdrawal.account), &account)
            .await;

        Ok(meta)
    }

    async fn validate_output(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        deposit: &PoolOutput,
    ) -> Result<TransactionItemAmount, ModuleError> {
        // TODO: Maybe some checks into minimum deposit amount?

        // check deposit does not result in balance overflow
        if let Some(account) = dbtx
            .get_value(&db::AccountBalanceKey(deposit.account))
            .await
        {
            if !account.can_add_amount(deposit.amount) {
                return Err(StabilityPoolError::DepositTooLarge).into_module_error_other();
            }
        }

        Ok(TransactionItemAmount {
            amount: deposit.amount,
            // TODO: Figure out fee logic
            fee: fedimint_core::Amount::ZERO,
        })
    }

    async fn apply_output<'a, 'b>(
        &'a self,
        dbtx: &mut ModuleDatabaseTransaction<'b, ModuleInstanceId>,
        deposit: &'a PoolOutput,
        outpoint: OutPoint,
    ) -> Result<TransactionItemAmount, ModuleError> {
        let txo_amount = self.validate_output(dbtx, deposit).await?;

        let mut account = dbtx
            .get_value(&db::AccountBalanceKey(deposit.account))
            .await
            .unwrap_or_default();
        account.unlocked.msats = account
            .unlocked
            .msats
            .checked_add(deposit.amount.msats)
            .expect("already checked overflow");

        dbtx.insert_entry(&db::AccountBalanceKey(deposit.account), &account)
            .await;

        dbtx.insert_new_entry(&db::DepositOutcomeKey(outpoint), &deposit.account)
            .await;

        Ok(txo_amount)
    }

    async fn end_consensus_epoch<'a, 'b>(
        &'a self,
        _consensus_peers: &HashSet<PeerId>,
        _dbtx: &mut ModuleDatabaseTransaction<'b, ModuleInstanceId>,
    ) -> Vec<PeerId> {
        vec![]
    }

    async fn output_status(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        outpoint: OutPoint,
    ) -> Option<PoolOutputOutcome> {
        dbtx.get_value(&db::DepositOutcomeKey(outpoint))
            .await
            .map(PoolOutputOutcome)
    }

    async fn audit(
        &self,
        dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        audit: &mut Audit,
    ) {
        audit
            .add_items(dbtx, &AccountBalanceKeyPrefix, |_, v| {
                ((v.unlocked + v.locked.amount()).msats) as i64
            })
            .await;
    }

    fn api_endpoints(&self) -> Vec<ApiEndpoint<Self>> {
        api::endpoints()
    }
}

impl StabilityPool {
    /// Create new module instance
    pub fn new(cfg: PoolConfig) -> Self {
        let oracle = cfg.consensus.oracle.oracle_client();
        Self {
            cfg,
            oracle,
            backoff: Default::default(),
            proposed_db: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum StabilityPoolError {
    SomethingDummyWentWrong,
    DepositTooLarge,
}

impl std::fmt::Display for StabilityPoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SomethingDummyWentWrong => write!(f, "placeholder error"),
            Self::DepositTooLarge => write!(f, "that deposit pukking big"),
        }
    }
}

impl std::error::Error for StabilityPoolError {}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WithdrawalError {
    UnavaliableFunds {
        amount: fedimint_core::Amount,
        avaliable: fedimint_core::Amount,
    },
}

impl std::fmt::Display for WithdrawalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WithdrawalError::UnavaliableFunds { amount, avaliable } => write!(
                f,
                "attempted to withdraw {} when only {} was avaliable",
                amount, avaliable
            ),
        }
    }
}

impl std::error::Error for WithdrawalError {}
