use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::common::TestEnvironment;

#[tokio::test]
async fn test_meroctl_stream_install() {
    let env = TestEnvironment::setup().await;

    // TODO:IMPLTEST
    // Test actual meroctl command with stream input
    // Verify application is installed on node
    // Test both pipe input and file input scenarios
}
