Account Monitor
===============

> Locally monitor accounts on EVM chains, without leaking information to RPC providers.

# Why?
The goal of this project is to notify users if something unexpected happened (account was compromised), in a way that preserves privacy from 3rd party services.

# Limitations
* For native token transfers, only outgoing transactions are detected (wen [EIP-7708](https://eip.tools/eip/7708)?) and those, only when using `Blocks` mode.
* Smart contract wallets will not trigger any notifications when sending native tokens, even in `Blocks` mode.
* While some types of transactions are properly identified, "complex" transactions (swaps/buy) will not be correctly categorized, but a notification will be sent (depending on spam filter options).
* The token symbols come from [rotki assets](https://github.com/rotki/assets) which means that not all tokens are included and the list is only updated when Account Monitor is updated.
* No notifications for previous transactions.

# Features
* Support any EVM chain (at least in `Events` mode).
* Use [ntfy](https://ntfy.sh/) to allow self-hosted notifications.
* Load accounts via a yaml on start and/or via an API call.
* Script to load accounts from [rotki](https://rotki.com/).
* Prometheus monitoring endpoint.

# Quickstart

Requirements: docker

1. Clone this repository
2. Copy `.env.example` to `.env`
3. Configure variables in `.env` (requires an Alchemy API key and ntfy token)
4. Copy `accounts.example.yaml` to `accounts.yaml`
5. Modify `accounts.yaml` to remove the examples and add the accounts to monitor
6. Run `docker-compose up`

The instructions above use Alchemy to make them easy to follow, if you want to use another RPC provider (or even better, your own node), set the appropriate endpoint for `CHAIN_RPC_ETHEREUM` in `docker-compose.yaml`

# Configuration
Configuration is done via environment variables

## Global

| Variable             | Type     | Required | Description                                                                                                                                                                                |
| ---                  | ---      | ---      | ---                                                                                                                                                                                        |
|`NTFY_TOKEN`          | `string` | `true`   | Ntfy's Auth token                                                                                                                                                                          |
|`NTFY_URL`            | `string` | `true`   | Ntfy's server URL                                                                                                                                                                          |
|`NTFY_TOPIC`          | `string` | `true`   | Topic to send notifications to                                                                                                                                                             |
|`CHAINS`              | `string` | `true`   | Uppercase comma separated list of chains to monitor (any EVM chain is supported)                                                                                                           |
|`STATIC_ACCOUNTS_PATH`| `string` | `false`  | Location from which to read the accounts to add during launch. This should be a yaml file with the same format as `accounts.example.yaml`. If not set, all accounts must be added via REST |

## Per Chain
For each chain defined in `CHAINS` there should be a block with the following variables, with the defined suffix (`ETHEREUM` in this example)

| Variable                           | Type                                              | Required | Default       | Description                                                                                                                                            |
| ---                                | ---                                               | ---      | ---           | ---                                                                                                                                                    |
|`CHAIN_RPC_ETHEREUM`                | `string`                                          | `true`   |               | HTTPS RPC of a chain                                                                                                                                   |
| `CHAIN_NAME_ETHEREUM`              | `string`                                          | `true`   |               | Used in the notifications' message                                                                                                                     |
| `CHAIN_BLOCKTME_ETHEREUM`          | `int`                                             | `true`   |               | Milliseconds in between blocks. When using `Event` mode, increasing this value will make fewer requests to the RPC, batching all blocks in an interval |
| `CHAIN_MODE_ETHEREUM`              | `Blocks &#124; Events`                            | `false`  | `Blocks`      | Method to use when queering RPCs for new transactions. See [Mode](#mode)                                                                               |
| `CHAIN_SPAM_FILTER_LEVEL_ETHEREUM` | `None &#124; KnownAssets &#124; SelfSubmittedTxs` | `false`  | `KnownAssets` | Spam filter configuration for the chain, see [Spam Filter](#spam-filter)                                                                               |
| `CHAIN_EXPLORER_ETHEREUM`          | `string`                                          | `false`  | `None`        | Domain of the chain's explorer, to include a link in the notification                                                                                  |
| `CHAIN_ID_ETHEREUM`                | `int`                                             | `false`  |               | Chain ID. Only used for verification, will be ignored if not configured                                                                                |

### Mode
One of the goals of this project is to be able to monitor accounts across all the EVM chains a user wants for free using an RPC provider. The two modes have important trade-offs, choose carefully. Neither method leaks any of the monitored accounts to the RPC providers.
* **Events**: The events mode will **not** include outgoing transfers of native tokens nor transactions which do not emit any onchain Events/Logs, but setting a `CHAIN_BLOCKTME_chain` higher than the actual chain blocktime (5000 for 5s) allows users to scrape 6~7 chains using a free Alchemy account.
* **Blocks**: Blocks mode uses a more expensive method to query RPCs but does include all outgoing transactions even if they only send native tokens or don't have any Events/Logs. Not all RPC Provider/Chains support this mode as it uses the newish method `eth_getBlockReceipts` (or Alchemy's version `alchemy_getTransactionReceipts`)

### Spam Filter
Chains with cheap gas cause a lot of incoming spam/scam transactions. `CHAIN_SPAM_FILTER_LEVEL_chain` can be used to filter out unwanted notifications. The available options are: (from strict to noisy)
* **SelfSubmittedTxs**: Only transactions sent by monitored accounts will trigger notifications. Will not notify of any incoming transactions. Not useful for use with Smart Contract Wallets.
* **KnownAssets**: Transactions that pass SelfSubmittedTxs + Transactions where a known token (from [rotki assets](https://github.com/rotki/assets)) is transferred to or from a monitored account.
* **None**: All transactions will trigger notifications.

## Debugging configuration
The following environment variables can be used to debug Account Monitor


| Variable       | Type      | Required | Default | Description                                                                                                                                |
| ---            | ---       | ---      | ---     | ---                                                                                                                                        |
| `RUST_LOG`     | `string`  | `false`  |         | Use `account_monitor=debug` to enable debugging                                                                                            |
| `NTFY_DISABLE` | `boolean` | `false`  | `false` | Log notification message instead of sending it through ntfy. Makes ntfy env variables optional. RUST_LOG should be at least set to `info`. |
| `DEBUG_BLOCK`  | `int`     | `false`  |         | Look for transactions in a single block. The program will exit when a transaction of a monitored account is found.                         |

# API
The API exposes a single endpoint to add a monitored account. There is no way to list the currently monitored accounts. Attempts to add an already monitored account, will be ignored.

A reverse proxy can be used to manage access to the API endpoint.

The following command can be used to add a new account:
```sh
curl --json '{"address":"0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", "label":"Vitalik"}' http://localhost:3030
```

# Scripts
A couple of helper scripts are available to facilitate adding accounts via the API.
Both scripts need `LOADING_SCRIPTS_HOST` to be set (or included in `.env`) this should point to where Account Monitor is running.

* `scripts/load_accounts_from_rotki.sh`: Requires rotki to be open and logged in. Will monitor all the user's Ethereum addresses setup in rotki.
* `scripts/load_accounts_from_yaml.sh`: Receives the location of a yaml file with accounts as a parameter. The file should have the same format as [accounts.example.yaml](./accounts.example.yaml).

# Acknowledgments
Thanks to [rotki](https://rotki.com) for curating tokens and known addresses. And an awesome portfolio tracker.

Thanks to [ntfy](https://ntfy.sh) for a building a simple to use notification server, compatible with computers, and phones with most OSs.

# Contributing
Issues and PRs for this repository are welcome and managed in [Radicle](https://radicle.xyz/guides/user).

Repository ID: [rad:z3SRsZ8YdvUg4UEkqffuHrFdFEcML](https://app.radicle.xyz/nodes/seed.radicle.garden/rad:z3SRsZ8YdvUg4UEkqffuHrFdFEcML)
