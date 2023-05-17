use std::ffi;
use std::sync::Arc;

use crate::common::stability_core::{self, CollateralRatio};
use crate::common::{self, ActionStaged, EpochOutcome, ProviderBid, SeekerAction};
use crate::{PoolClientExt, PoolClientModule};
use clap::{Parser, Subcommand};
use common::PoolStateMachine;
use fedimint_client::sm::OperationId;
use fedimint_client::transaction::{ClientInput, ClientOutput, TransactionBuilder};
use fedimint_client::Client;
use fedimint_core::core::IntoDynInstance;
use fedimint_core::{Amount, OutPoint, TransactionId};
use stabilitypool_server::api::{BalanceResponse, SideResponse};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct PoolCli {
    #[clap(subcommand)]
    command: PoolCommand,
}

#[derive(Subcommand, Clone)]
pub enum PoolCommand {
    /// Get stability pool account details.
    Balance,

    /// Get outcome of given stability pool epoch.
    Epoch { epoch_id: u64 },

    /// Get the next stability pool epoch.
    EpochNext,

    /// Deposit into unlocked balance of stability pool.
    Deposit { amount: Amount },

    /// Withdraw from unlocked balance of stability pool.
    Withdraw { amount: Amount },

    /// User action commands.
    #[clap(subcommand)]
    Action(Propose),

    /// Get the state of the stability pool.
    State,
}

#[derive(Subcommand, Clone)]
pub enum Propose {
    /// Check the current action staged for account.
    Staged,
    /// Lock funds as a seeker.
    SeekerLock { amount: Amount },
    /// Unlock funds as a seeker.
    SeekerUnlock { amount: Amount },
    /// Bid as a provider.
    ProviderBid { min_feerate: u64, amount: Amount },
}

#[derive(serde::Serialize)]
#[serde(rename_all(serialize = "snake_case"))]
pub enum PoolCliOutput {
    Balance {
        balance: AccountBalance,
    },

    EpochOutcome {
        outcome: EpochOutcome,
    },

    EpochNext {
        epoch_id: u64,
    },

    Deposit {
        deposit_tx: OutPoint,
    },

    Withdraw {
        withdraw_tx: TransactionId,
    },

    Propose {},

    Staged {
        action: ActionStaged,
    },

    State {
        state: stabilitypool_server::api::State,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AccountBalance {
    pub unlocked: u64,
    pub locked: Option<LockedBalance>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all(serialize = "snake_case"))]
pub struct LockedBalance {
    side: SideResponse,
    locked_value_msat: u64,
    locked_value_usd: f64,
    current_value_msat: u64,
    current_value_usd: f64,
    epoch_fee_msat: i64,
    epoch_fee_usd: f64,
    feerate: u64,
    msat_pnl: i64,
    usd_pnl: f64,
    current_price: f64,
    epoch_start_price: f64,
}

impl LockedBalance {
    pub fn from_core_locked_balance(
        balance: stabilitypool_server::api::LockedBalanceResponse,
        current_price: u64,
        collateral_ratio: CollateralRatio,
    ) -> Self {
        let feerate = balance.epoch.feerate;
        let epoch_start_price: u64 = balance.epoch_start_price;
        let side = balance.side;
        let locked_value_msat = balance.value;
        let current_value_msat = match side {
            SideResponse::Provider => stability_core::provider_payout(
                locked_value_msat,
                feerate,
                epoch_start_price,
                current_price,
                collateral_ratio,
            ),
            SideResponse::Seeker => stability_core::seeker_payout(
                locked_value_msat,
                feerate,
                epoch_start_price,
                current_price,
                collateral_ratio,
            ),
        };
        let epoch_fee_msat = match side {
            SideResponse::Provider => {
                -(stability_core::provider_fee(feerate, locked_value_msat, collateral_ratio) as i64)
            }
            SideResponse::Seeker => stability_core::seeker_fee(feerate, locked_value_msat) as i64,
        };

        let msat_pnl = locked_value_msat as i64 - current_value_msat as i64;
        LockedBalance {
            side,
            locked_value_msat,
            locked_value_usd: msat_to_usd(locked_value_msat as i64, epoch_start_price),
            current_value_msat,
            current_value_usd: msat_to_usd(current_value_msat as i64, current_price),
            epoch_fee_msat,
            epoch_fee_usd: msat_to_usd(epoch_fee_msat, current_price),
            feerate: feerate.approx_ppm_feerate(),
            msat_pnl,
            usd_pnl: msat_to_usd(msat_pnl, current_price),
            current_price: (current_price as f64) / 100.0,
            epoch_start_price: (epoch_start_price as f64) / 100.0,
        }
    }
}

fn msat_to_usd(msat: i64, price: u64) -> f64 {
    msat as f64 * (price as f64 / 100.0) / 1e11
}

pub(crate) async fn handle_cli_args(
    client: &Client,
    rng: rand::rngs::OsRng,
    args: &[ffi::OsString],
) -> anyhow::Result<PoolCliOutput> {
    let args = PoolCli::try_parse_from(args)?;
    handle_command(client, args.command, rng).await
}

pub(crate) async fn handle_command(
    client: &Client,
    command: PoolCommand,
    _rng: rand::rngs::OsRng,
) -> anyhow::Result<PoolCliOutput> {
    let (pool_client, pool_instance) = client.get_first_module::<PoolClientModule>(&common::KIND);
    let oracle = pool_client.cfg.oracle.oracle_client();

    match command {
        PoolCommand::Balance => {
            let balance: BalanceResponse = client.balance().await?;
            let current_price = oracle.price_now().await.unwrap();
            let output = AccountBalance {
                unlocked: balance.unlocked,
                locked: balance.locked.map(|balance| {
                    LockedBalance::from_core_locked_balance(
                        balance,
                        current_price,
                        pool_client.cfg.collateral_ratio,
                    )
                }),
            };
            Ok(PoolCliOutput::Balance { balance: output })
        }
        PoolCommand::Epoch { epoch_id } => {
            let outcome = client.epoch_outcome(epoch_id).await?;
            Ok(PoolCliOutput::EpochOutcome { outcome })
        }
        // The minimum oldest epoch id that the client can act on
        PoolCommand::EpochNext => {
            let epoch_id = client.staging_epoch().await?;
            Ok(PoolCliOutput::EpochNext { epoch_id })
        }
        PoolCommand::Deposit { amount } => {
            // let tx = TransactionBuilder::default();
            // let pool_dbtx = pool_instance.db.begin_transaction().await;
            // let mut mint_dbtx = mint_instance.db.begin_transaction().await;
            let op_id = OperationId(rand::random());

            // let input = mint_client
            //     .create_input(&mut mint_dbtx.get_isolated(), op_id, amount)
            //     .await?;

            let output = ClientOutput {
                output: common::AccountDeposit {
                    amount,
                    account: pool_client.key.x_only_public_key().0,
                },
                state_machines: Arc::new(move |_, _| Vec::<PoolStateMachine>::new()),
            };

            let tx = TransactionBuilder::new()
                // .with_input(input.into_dyn(pool_instance.id))
                .with_output(output.into_dyn(pool_instance.id));
            // .with_output(output.into_dyn(pool_instance.id));

            let outpoint = |txid| OutPoint { txid, out_idx: 0 };

            let txid = client
                .finalize_and_submit_transaction(op_id, common::KIND.as_str(), outpoint, tx)
                .await?;

            Ok(PoolCliOutput::Deposit {
                deposit_tx: OutPoint { txid, out_idx: 0 },
            })
        }
        PoolCommand::Withdraw { amount } => {
            // let tx = TransactionBuilder::default();
            // let pool_dbtx = pool_instance.db.begin_transaction().await;
            let op_id = OperationId(rand::random());

            let input = ClientInput {
                input: common::AccountWithdrawal {
                    amount,
                    account: pool_client.key.x_only_public_key().0,
                },
                state_machines: Arc::new(move |_, _| Vec::<PoolStateMachine>::new()),
                keys: vec![pool_client.key],
            };

            // let output = ClientOutput {
            //     output: common::AccountDeposit {
            //         amount,
            //         account: pool_client.key.x_only_public_key().0,
            //     },
            //     state_machines: Arc::new(move |_, _| Vec::<PoolStateMachine>::new()),
            // };

            let tx = TransactionBuilder::new().with_input(input.into_dyn(pool_instance.id));
            // .with_output(output.into_dyn(pool_instance.id));

            let outpoint = |txid| OutPoint { txid, out_idx: 0 };

            let txid = client
                .finalize_and_submit_transaction(op_id, common::KIND.as_str(), outpoint, tx)
                .await?;

            Ok(PoolCliOutput::Withdraw { withdraw_tx: txid })
        }
        PoolCommand::Action(action) => {
            let res = match action {
                Propose::Staged => {
                    return Ok(PoolCliOutput::Staged {
                        action: client.staged_action().await?,
                    })
                }
                Propose::SeekerLock { amount } => {
                    client
                        .propose_seeker_action(SeekerAction::Lock { amount })
                        .await
                }
                Propose::SeekerUnlock { amount } => {
                    client
                        .propose_seeker_action(SeekerAction::Unlock { amount })
                        .await
                }
                Propose::ProviderBid {
                    amount,
                    min_feerate,
                } => {
                    client
                        .propose_provider_action(ProviderBid {
                            max_amount: amount,
                            min_feerate,
                        })
                        .await
                }
            };

            match res {
                Ok(_) => Ok(PoolCliOutput::Propose {}),
                Err(e) => Err(e.into()),
            }
        }
        PoolCommand::State => match client.state().await {
            Ok(state) => Ok(PoolCliOutput::State { state }),
            Err(e) => Err(e.into()),
        },
    }
}
