// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract ContextConfig {
    mapping(bytes32 => bytes) private config;

    event ConfigSet(bytes32 indexed key, bytes value);

    function setConfig(bytes32 key, bytes memory value) external {
        config[key] = value;
        emit ConfigSet(key, value);
    }

    function getConfig(bytes32 key) external view returns (bytes memory) {
        return config[key];
    }
} 