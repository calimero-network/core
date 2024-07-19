# Calimero Identity

- [Introduction](#introduction)
- [Auth Functions](#functions)
  - [Verify NEAR signature](#verify_near_signature)
  - [Verify ETH signature](#verify_eth_signature)

## Introduction

Decentralized identity, also referred to as self-sovereign identity, is an
open-standards based identity framework that uses digital identifiers and
verifiable credentials that are self-owned, independent, and enable trusted data
exchange.

This library provides basic functionalities for using decentralized identity

## Functions

### Verify NEAR signature

Function `verify_near_public_key` verifies NEAR public keys by decoding a
base58-encoded public key and validating a signed message against it.

Parameters:

- `public_key`: A base58-encoded string representing the NEAR public key.
- `msg`: The message that was signed.
- `signature`: The signature to verify.

### Verify ETH signature

Function `verify_eth_signature` verifies Ethereum signatures by recovering the
public key from a signed message and comparing it to a provided account address.

Parameters:

- `account`: The Ethereum account address.
- `message`: The original message that was signed.
- `signature`: The signature to verify.
