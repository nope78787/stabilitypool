pub mod api;

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use fedimint_core::config::{
    ConfigGenParams, DkgResult, ModuleConfigResponse, ModuleGenParams, ServerModuleConfig,
    TypedServerModuleConfig, TypedServerModuleConsensusConfig,
};
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::db::{Database, DatabaseVersion, ModuleDatabaseTransaction};
use fedimint_core::encoding::Encodable;
use fedimint_core::module::__reexports::serde_json;
use fedimint_core::module::audit::Audit;
use fedimint_core::module::interconnect::ModuleInterconect;
use fedimint_core::module::{
    ApiEndpoint, ApiVersion, ConsensusProposal, CoreConsensusVersion, ExtendsCommonModuleGen,
    InputMeta, IntoModuleError, ModuleConsensusVersion, ModuleError, PeerHandle, ServerModuleGen,
    TransactionItemAmount,
};
use fedimint_core::server::DynServerModule;
use fedimint_core::task::TaskGroup;
use fedimint_core::{NumPeers, OutPoint, PeerId, ServerModule};
use serde::{Deserialize, Serialize};
use stabilitypool::db::AccountBalanceKeyPrefix;
use stabilitypool::stability_core::CollateralRatio;

use stabilitypool::common::PoolModuleTypes;
use stabilitypool::config::{
    EpochConfig, OracleConfig, PoolConfig, PoolConfigConsensus, PoolConfigPrivate,
};
use stabilitypool::{
    db, ActionProposedDb, BackOff, ConsensusItemOutcome, OracleClient, PoolCommonGen,
    PoolConsensusItem, PoolInput, PoolOutput, PoolOutputOutcome,
};

use stabilitypool::action;
use stabilitypool::epoch;
// pub use stabilitypool::epoch::*;
// pub use stabilitypool::price::*;

// The default global max feerate.
// TODO: Have this actually in config.
pub const DEFAULT_GLOBAL_MAX_FEERATE: u64 = 100_000;

/// The default epoch length is 24hrs (represented in seconds).
// pub const DEFAULT_EPOCH_LENGTH: u64 = 24 * 60 * 60;
pub const DEFAULT_EPOCH_LENGTH: u64 = 40; // TODO: This is just for testing

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfigGenParams {
    pub important_param: u64,
    #[serde(default)]
    pub start_epoch_at: Option<time::PrimitiveDateTime>,
    /// this is in seconds
    pub epoch_length: u64,
    pub oracle_config: OracleConfig,
    /// The ratio of seeker position to provider collateral
    #[serde(default)]
    pub collateral_ratio: CollateralRatio,
}

impl ModuleGenParams for PoolConfigGenParams {
    const MODULE_NAME: &'static str = "stabilitypool";
}

impl Default for PoolConfigGenParams {
    fn default() -> Self {
        Self {
            important_param: 3,
            start_epoch_at: None,
            epoch_length: DEFAULT_EPOCH_LENGTH,
            oracle_config: OracleConfig::default(),
            collateral_ratio: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolConfigGenerator;

impl ExtendsCommonModuleGen for PoolConfigGenerator {
    type Common = PoolCommonGen;
}

#[async_trait]
impl ServerModuleGen for PoolConfigGenerator {
    const DATABASE_VERSION: DatabaseVersion = DatabaseVersion(1);

    fn versions(&self, _core: CoreConsensusVersion) -> &[ModuleConsensusVersion] {
        &[ModuleConsensusVersion(0)]
    }

    async fn init(
        &self,
        cfg: ServerModuleConfig,
        _db: Database,
        _env: &BTreeMap<OsString, OsString>,
        _task_group: &mut TaskGroup,
    ) -> anyhow::Result<DynServerModule> {
        Ok(StabilityPool::new(cfg.to_typed()?).into())
    }

    fn trusted_dealer_gen(
        &self,
        peers: &[PeerId],
        params: &ConfigGenParams,
    ) -> BTreeMap<PeerId, ServerModuleConfig> {
        let params = params
            .get::<PoolConfigGenParams>()
            .expect("Invalid mint params");

        let mint_cfg: BTreeMap<_, PoolConfig> = peers
            .iter()
            .map(|&peer| {
                let config = PoolConfig {
                    private: PoolConfigPrivate { peer_id: peer },
                    consensus: PoolConfigConsensus {
                        epoch: EpochConfig {
                            start_epoch_at: params
                                .start_epoch_at
                                .map(|prim_datetime| prim_datetime.assume_utc())
                                .unwrap_or_else(|| time::OffsetDateTime::now_utc())
                                .unix_timestamp() as _,
                            epoch_length: params.epoch_length,
                            price_threshold: peers.threshold() as _,
                            max_feerate_ppm: DEFAULT_GLOBAL_MAX_FEERATE,
                            collateral_ratio: params.collateral_ratio,
                        },
                        oracle: params.oracle_config.clone(),
                    },
                };
                (peer, config)
            })
            .collect();

        mint_cfg
            .into_iter()
            .map(|(k, v)| (k, v.to_erased()))
            .collect()
    }

    async fn distributed_gen(
        &self,
        peers: &PeerHandle,
        params: &ConfigGenParams,
    ) -> DkgResult<ServerModuleConfig> {
        let params = params
            .get::<PoolConfigGenParams>()
            .expect("Invalid mint params");

        let server = PoolConfig {
            private: PoolConfigPrivate {
                peer_id: peers.our_id,
            },
            consensus: PoolConfigConsensus {
                epoch: EpochConfig {
                    start_epoch_at: params
                        .start_epoch_at
                        .map(|prim_datetime| prim_datetime.assume_utc())
                        .unwrap_or_else(|| time::OffsetDateTime::now_utc())
                        .unix_timestamp() as _,
                    epoch_length: params.epoch_length,
                    price_threshold: peers.peers.threshold() as _,
                    max_feerate_ppm: DEFAULT_GLOBAL_MAX_FEERATE,
                    collateral_ratio: params.collateral_ratio,
                },
                oracle: params.oracle_config,
            },
        };

        Ok(server.to_erased())
    }

    fn to_config_response(
        &self,
        config: serde_json::Value,
    ) -> anyhow::Result<fedimint_core::config::ModuleConfigResponse> {
        let config = serde_json::from_value::<PoolConfigConsensus>(config)?;

        Ok(ModuleConfigResponse {
            client: config.to_client_config(),
            consensus_hash: config.consensus_hash()?,
        })
    }

    fn validate_config(&self, identity: &PeerId, config: ServerModuleConfig) -> anyhow::Result<()> {
        config.to_typed::<PoolConfig>()?.validate_config(identity)
    }

    async fn dump_database(
        &self,
        _dbtx: &mut ModuleDatabaseTransaction<'_, ModuleInstanceId>,
        _prefix_names: Vec<String>,
    ) -> Box<dyn Iterator<Item = (String, Box<dyn erased_serde::Serialize + Send>)> + '_> {
        Box::new(BTreeMap::new().into_iter())
    }
}

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
    type Gen = PoolConfigGenerator;
    type Common = PoolModuleTypes;
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
        crate::api::endpoints()
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
