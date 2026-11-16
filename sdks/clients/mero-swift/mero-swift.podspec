# mero-swift on CocoaPods. The pod is `mero-swift`; the module it builds is
# still `MeroKit`, so `import MeroKit` is unchanged. The version is core's
# (the root VERSION of calimero-network/core; scripts/sdks.py version-sync
# writes it here), and the source is the read-only SwiftPM mirror,
# calimero-network/swift-sdk, which core's release tags with that version.
Pod::Spec.new do |spec|
  spec.name         = 'mero-swift'
  spec.module_name  = 'MeroKit'
  spec.version      = '1.0.0-rc.30'
  spec.summary      = 'Native Swift SDK for building iOS apps against a remote Calimero node.'
  spec.description  = <<-DESC
    mero-swift (module MeroKit) is the Swift port of the Calimero mero-js
    CalimeroClient: attestation, the sealed channel, passkeys, organizations,
    teams, app instances and live events, over async/await.
  DESC
  spec.homepage     = 'https://github.com/calimero-network/swift-sdk'
  spec.license      = { :type => 'MIT', :file => 'LICENSE' }
  spec.author       = { 'Calimero' => 'dev@calimero.network' }
  spec.source       = { :git => 'https://github.com/calimero-network/swift-sdk.git', :tag => spec.version.to_s }

  spec.swift_version         = '5.9'
  spec.ios.deployment_target = '15.0'
  spec.osx.deployment_target = '12.0'

  # Core SDK only. The SwiftUI layer (MeroKitUI) and the sample app are SPM-only.
  spec.source_files = 'Sources/MeroKit/**/*.swift'
  spec.frameworks   = 'Foundation', 'Security'
end
