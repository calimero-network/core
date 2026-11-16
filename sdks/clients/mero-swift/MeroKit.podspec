Pod::Spec.new do |spec|
  spec.name         = 'MeroKit'
  spec.version      = '0.1.0'
  spec.summary      = 'Native Swift SDK for building iOS apps against a remote Calimero node.'
  spec.description  = <<-DESC
    MeroKit is a faithful Swift port of the Calimero mero-js wire contract:
    authentication with single-use refresh-token handling, JSON-RPC contract
    calls, the admin API, and SSO deep-link login — all over async/await.
  DESC
  spec.homepage     = 'https://github.com/calimero-network/swift-sdk'
  spec.license      = { :type => 'MIT', :file => 'LICENSE' }
  spec.author       = { 'Calimero' => 'dev@calimero.network' }
  spec.source       = { :git => 'https://github.com/calimero-network/swift-sdk.git', :tag => "v#{spec.version}" }

  spec.swift_version         = '5.9'
  spec.ios.deployment_target = '15.0'
  spec.osx.deployment_target = '12.0'

  # Core SDK only. The SwiftUI layer (MeroKitUI) and the sample app are SPM-only.
  spec.source_files = 'Sources/MeroKit/**/*.swift'
  spec.frameworks   = 'Foundation', 'Security'
end
