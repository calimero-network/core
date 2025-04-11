// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "forge-std/Script.sol";
import "../src/ContextConfig.sol";

contract DeployScript is Script {
    function run() external {
        // Use default test private key
        uint256 deployerPrivateKey = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
        vm.startBroadcast(deployerPrivateKey);
        
        ContextConfig config = new ContextConfig();
        console.log("ContextConfig deployed at:", address(config));
        
        // Save the contract address to a file
        string memory json = vm.serializeAddress("deployment", "address", address(config));
        vm.writeJson(json, "deployments/localhost/ContextConfig.json");
        
        vm.stopBroadcast();
    }
} 