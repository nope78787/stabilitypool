# shellcheck shell=bash

FEDIMINT_DIR=../fedimint


### Setup test atop of fedimint build script
source $FEDIMINT_DIR/scripts/build.sh
export FM_CFG_DIR="$FM_TEST_DIR/cfg"

# Define our own clients
mkdir $FM_CFG_DIR/seeker
mkdir $FM_CFG_DIR/provider_a
mkdir $FM_CFG_DIR/provider_b
cp $FM_CFG_DIR/client.json $FM_CFG_DIR/seeker/client.json
cp $FM_CFG_DIR/client.json $FM_CFG_DIR/provider_a/client.json
cp $FM_CFG_DIR/client.json $FM_CFG_DIR/provider_b/client.json
export FM_S_CLIENT="fedimint-cli --workdir $FM_CFG_DIR/seeker"
export FM_A_CLIENT="fedimint-cli --workdir $FM_CFG_DIR/provider_a"
export FM_B_CLIENT="fedimint-cli --workdir $FM_CFG_DIR/provider_b"

export FM_S_CLIENT="fedimint-cli --workdir $FM_CFG_DIR/seeker"
export FM_A_CLIENT="fedimint-cli --workdir $FM_CFG_DIR/provider_a"
export FM_B_CLIENT="fedimint-cli --workdir $FM_CFG_DIR/provider_b"
###

### Do usual peg-ins
source $FEDIMINT_DIR/scripts/pegin.sh

# And more peg-ins for our new clients
PEGINADDRESS=$(seeker-cli peg-in-address | jq -r '.address')
# this default amount suggested by copilot, open to changes
PEGINTXID=$(send_bitcoin $PEGINADDRESS 100000000)
mine_blocks 11
TXOUTPROOF=$(get_txout_proof $PEGINTXID)
TRANSACTION=$(get_raw_transaction $PEGINTXID)
seeker-cli peg-in $TXOUTPROOF $TRANSACTION
seeker-cli fetch
seeker-cli info

PEGINADDRESS=$(provider-cli peg-in-address | jq -r '.address')
# this default amount suggested by copilot, open to changes
PEGINTXID=$(send_bitcoin $PEGINADDRESS 100000000)
mine_blocks 11
TXOUTPROOF=$(get_txout_proof $PEGINTXID)
TRANSACTION=$(get_raw_transaction $PEGINTXID)
provider-cli peg-in $TXOUTPROOF $TRANSACTION
provider-cli fetch
provider-cli info

# test these: pool-staged-seeker-action, pool-staged-provider-bid, pool-balance, pool-epoch-outcome, pool-staging-epoch, pool-deposit, pool-withdraw, pool-action

provider-cli pool-deposit 80000000
provider-cli pool-balance
provider-cli pool-withdraw 5000000

seeker-cli pool-deposit 80000000
seeker-cli pool-balance

THIS_EPOCH=$(seeker-cli pool-staging-epoch | jq -r '.epoch_id')
LAST_EPOCH=$(($THIS_EPOCH - 1))
seeker-cli pool-epoch-outcome $LAST_EPOCH