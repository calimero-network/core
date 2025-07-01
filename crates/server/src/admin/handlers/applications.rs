#[cfg(test)]
mod tests {
    // use tokio_stream::StreamExt;

    // TODO:IMPLTEST

    use super::*;

    #[tokio::test]
    async fn test_install_application_stream_with_valid_hash() {
        // Test stream installation with hash verification
    }

    #[tokio::test]
    async fn test_install_application_stream_size_validation() {
        // Test size validation
    }

    #[tokio::test]
    async fn test_install_application_stream_invalid_data() {
        // Test error handling for corrupted streams
    }
}

pub mod get_application;
pub mod install_application;
pub mod install_application_stream;
pub mod install_dev_application;
pub mod list_applications;
pub mod uninstall_application;
