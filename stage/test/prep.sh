#!/usr/bin/env bash
set -e

cd "$(dirname $0)"

export RUST_LOG=calimero_=debug,calimero_network=error,calimero_node::handlers::network_event=error

cargo build -p merod -p meroctl

export CALIMERO_HOME=.

merod="../../target/debug/merod"
meroctl="../../target/debug/meroctl"

${merod} --node-name n1 init --server-port 2550 --swarm-port 2450 --force
${merod} --node-name n2 init --server-port 2551 --swarm-port 2451 --force

TFREQ=6000
TINTVL=5000
TSIGNER="self"
TCONTRACT="dev-20250723152516-51710367534578"
TRPC="http://127.0.0.1:57154/"
TACCOUNT="node1.test.near"
TPUBKEY="ed25519:Gxa24TGbJu4mqdhW3GbvLXmf4bSEyxVicrtpChDWbgga"
TSECKEY="ed25519:3JtQnV5Tm5GM35t24mytoqR4UbLEAa6km4tiPXd6ubXebCrviQ7usSWJNKFYNyFkmtf6D2qZfN9ZUw8C2mibXw1C"

for n in n1 n2; do
  ${merod} --node-name $n config \
    sync.frequency_ms=${TFREQ} \
    sync.interval_ms=${TINTVL} \
    bootstrap.nodes='[]' \
    context.config.near.signer=\"${TSIGNER}\" \
    context.config.near.contract_id=\"${TCONTRACT}\" \
    context.config.signer.self.near.testnet.rpc_url=\"${TRPC}\" \
    context.config.signer.self.near.testnet.account_id=\"${TACCOUNT}\" \
    context.config.signer.self.near.testnet.public_key=\"${TPUBKEY}\" \
    context.config.signer.self.near.testnet.secret_key=\"${TSECKEY}\"
done

PID1= PID2=

stop() {
  pid="PID$1"
  if [ -n "${!pid}" ]; then
    echo "Stopping n$1"
    kill -SIGTERM "${!pid}"
    wait "${!pid}" || true
    export "${pid}"=
  fi
}

cleanup() {
  stop 1
  stop 2
}
trap cleanup EXIT

stdin() {
  if [ ! -p n$1.i ]; then
    mkfifo n$1.i
  fi
}

scoped() {
  sed "s/^/($3n$1\x1b[39m|$2) /"
}

run() {
  local fg
  if [ "$1" -eq 1 ]; then
    fg="\x1b[32m"
  elif [ "$1" -eq 2 ]; then
    fg="\x1b[36m"
  fi

  stdin $1

  <n$1.i ${merod} --node-name n"$1" run 2> >(scoped $1 2 $fg >&1) > >(scoped $1 1 $fg) &
  export PID$1=$!
}

run 1
run 2

sleep 1

app_id=`${meroctl} --node n1 --output-format json app install --path ../../apps/kv-store/res/kv_store.wasm | jq -r .data.applicationId`

${meroctl} --node n2 app install --path ../../apps/kv-store/res/kv_store.wasm

create_response=`${meroctl} --node n1 --output-format json context create --application-id $app_id --protocol near --name default --as default | jq -s first`

echo "Create response: $create_response"

ctx=`jq <<< "$create_response" -r '.data.contextId'`
usr1=`jq <<< "$create_response" -r '.data.memberPublicKey'`

echo "Context: $ctx"
echo "User 1: $usr1"

usr2=`${meroctl} --node n2 --output-format json context identity generate | jq -r '.data.publicKey'`

echo "User 2: $usr2"

invite=`${meroctl} --node n1 --output-format json context invite ${usr2} | jq -r '.data'`

echo "Invitation: $invite"

${meroctl} --node n2 context join ${invite} --name default --as default

${meroctl} --node n1 call set --args '["usr1","Jake"]'
sleep 3

${meroctl} --node n2 call set --args '["usr2","Barry"]'
sleep 3

${meroctl} --node n1 call entries
${meroctl} --node n2 call entries

echo "\x1b[32m IN SYNC? STOPPING 1 \x1b[0m"

stop 1

${meroctl} --node n2 call set --args '["usr3","Peggy"]'
${meroctl} --node n2 call entries
sleep 1

echo "\x1b[32m ADVANCED 2, STOPPING.. \x1b[0m"

stop 2

echo "\x1b[32m STARTING 1 \x1b[0m"

run 1

sleep 1

${meroctl} --node n1 call set --args '["usr4","Fred"]'
${meroctl} --node n1 call entries
sleep 1

echo "\x1b[32m ADVANCED 1, RESTARTING 2 \x1b[0m"

run 2

echo "\x1b[32m BOTH ONLINE, SHOULD SYNC.. \x1b[0m"

sleep 5

${meroctl} --node n1 call entries
${meroctl} --node n2 call entries

sleep 5000

# at this point you can use meroctl or `cat > n{1}.i` to send commands to the interactive CLI
