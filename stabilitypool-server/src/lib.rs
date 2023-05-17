pub mod api;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use common::config::{
    EpochConfig, PoolClientConfig, PoolConfig, PoolConfigConsensus, PoolConfigGenParams,
    PoolConfigPrivate,
};
use common::db::AccountBalanceKeyPrefix;
use common::PoolModuleTypes;
use common::{
    db, BackOff, ConsensusItemOutcome, OracleClient, PoolCommonGen, PoolConsensusItem, PoolInput,
    PoolOutput, PoolOutputOutcome,
};
use fedimint_core::config::{
    ClientModuleConfig, ConfigGenModuleParams, DkgResult, ServerModuleConfig,
    ServerModuleConsensusConfig, TypedServerModuleConfig, TypedServerModuleConsensusConfig,
};
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::db::{Database, DatabaseVersion, ModuleDatabaseTransaction};
use fedimint_core::module::audit::Audit;
use fedimint_core::module::interconnect::ModuleInterconect;
use fedimint_core::module::{
    ApiEndpoint, ConsensusProposal, CoreConsensusVersion, ExtendsCommonModuleGen, InputMeta,
    IntoModuleError, ModuleConsensusVersion, ModuleError, PeerHandle, ServerModuleGen,
    SupportedModuleApiVersions, TransactionItemAmount,
};
use fedimint_core::server::DynServerModule;
use fedimint_core::task::TaskGroup;
use fedimint_core::{NumPeers, OutPoint, PeerId, ServerModule};
use stabilitypool_common as common;

use common::action::{self, ActionProposedDb};
use common::epoch;

// The default global max feerate.
// TODO: Have this actually in config.
pub const DEFAULT_GLOBAL_MAX_FEERATE: u64 = 100_000;

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
        params: &ConfigGenModuleParams,
    ) -> BTreeMap<PeerId, ServerModuleConfig> {
        let params = params
            .to_typed::<PoolConfigGenParams>()
            .expect("Invalid mint params");

        let mint_cfg: BTreeMap<_, PoolConfig> = peers
            .iter()
            .map(|&peer| {
                let config = PoolConfig {
                    private: PoolConfigPrivate {},
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
        params: &ConfigGenModuleParams,
    ) -> DkgResult<ServerModuleConfig> {
        let params = params
            .to_typed::<PoolConfigGenParams>()
            .expect("Invalid mint params");

        let server = PoolConfig {
            private: PoolConfigPrivate {},
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

    fn validate_config(
        &self,
        _identity: &PeerId,
        config: ServerModuleConfig,
    ) -> anyhow::Result<()> {
        let _ = config.to_typed::<PoolConfig>()?;
        Ok(())
    }

    fn get_client_config(
        &self,
        config: &ServerModuleConsensusConfig,
    ) -> anyhow::Result<ClientModuleConfig> {
        let config = PoolConfigConsensus::from_erased(config)?;
        Ok(ClientModuleConfig::from_typed(
            config.kind(),
            config.version(),
            &PoolClientConfig {
                oracle: config.oracle,
                collateral_ratio: config.epoch.collateral_ratio,
            },
        )
        .expect("serialization can't fail 🤞"))
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

    fn supported_api_versions(&self) -> SupportedModuleApiVersions {
        SupportedModuleApiVersions::from_raw(0, 0, &[(0, 0)])
    }

    // fn versions(&self) -> (ModuleConsensusVersion, &[ApiVersion]) {
    //     (
    //         ModuleConsensusVersion(1),
    //         &[ApiVersion { major: 1, minor: 1 }],
    //     )
    // }

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
        dbtx: &mut ModuleDatabaseTransaction<'b>,
        consensus_items: Vec<(PeerId, PoolConsensusItem)>,
        _consensus_peers: &BTreeSet<PeerId>,
    ) -> Vec<PeerId> {
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

        vec![]
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
        _consensus_peers: &BTreeSet<PeerId>,
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
