use rcgen::{CertificateParams, DnType};
use local_ip_address::local_ip;
use std::fs;
use std::path::Path;
use x509_parser::prelude::*;
use x509_parser::extensions::GeneralName;
use std::path::PathBuf;

pub fn check_for_certificate() -> eyre::Result<()> {
    // Get the path to the certificate and key files
    let certificate_dir = get_certificate_dir();
    let cert_path = certificate_dir.join("cert.pem");
    let key_path = certificate_dir.join("key.pem");

    // Ensure paths exist
    if !cert_path.exists() || !key_path.exists() {
        generate_certificate()?;
        write_out_instructions();
        return Ok(());
    }

    // Check if a certificate contains local IP address
    check_certificate()?;
    write_out_instructions();
    Ok(())
}

fn generate_certificate() -> eyre::Result<()> {
    // Get the local IP address
    let local_ip = local_ip()?;

    // Generate a self-signed certificate for IP address and localhost
    let subject_alt_names = vec![
        local_ip.to_string(),
        "127.0.0.1".to_string(),
  	    "localhost".to_string()
    ];

    // Certificate name
    let certificate_name = format!("Calimero self-signed certificate for local IP: {:?}", &local_ip.to_string());

    // Create certificate parameters
    let mut params = CertificateParams::new(subject_alt_names.clone())?;
    // Set the distinguished name
    params.distinguished_name.push(DnType::CommonName, certificate_name);

    // Load keypair if it exists or generate a new one
    let certificate_dir = get_certificate_dir();
    let key_file_path = certificate_dir.join("key.pem");
    let key_pair = if key_file_path.exists() {
        let key_pem = fs::read_to_string(key_file_path)?;
        rcgen::KeyPair::from_pem(&key_pem)?
    }else {
        rcgen::KeyPair::generate().unwrap()
    };

    // Generate the certificate with the customized parameters
    let cert = CertificateParams::self_signed(params, &key_pair).unwrap();

    // Serialize the certificate and private key to PEM format
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    // Get the path to the certificate and key files
    let certificate_dir = get_certificate_dir();
    let cert_file_path = certificate_dir.join("cert.pem");
    let key_file_path = certificate_dir.join("key.pem");
    
    // Save the certificate and key to files
    fs::write(cert_file_path, cert_pem)?;
    fs::write(key_file_path, key_pem)?;

    Ok(())
}

fn delete_certificate() -> eyre::Result<()> {
    let certificate_dir = get_certificate_dir();
    let cert_path = certificate_dir.join("cert.pem");
    fs::remove_file(cert_path)?;

    Ok(())
}

fn check_certificate() -> eyre::Result<()> {
    
    let certificate_dir = get_certificate_dir();
    let cert_file = certificate_dir.join("cert.pem");
    // Read the certificate file
    let cert_pem = fs::read_to_string(cert_file)?;
    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())?;
    let x509_cert = pem.parse_x509()?;

    // Get the local IP address
    let local_ip = local_ip()?;

    // Parse the local IP address to Vec<u8>
    let local_ip_bytes = match local_ip {
        std::net::IpAddr::V4(ip) => ip.octets().to_vec(),
        std::net::IpAddr::V6(ip) => ip.octets().to_vec(),
    };

    // Flag for found IP address in certificate
    let mut ip_found = false;

    for ext in x509_cert.extensions() {
      // Parse the extension
      let parsed_ext = ext.parsed_extension();
      
      // Check if it's a SubjectAlternativeName extension
      if let ParsedExtension::SubjectAlternativeName(san) = parsed_ext {
          // Iterate over the general names
          for gn in &san.general_names {
              // Check if the general name is an IP address
              if let GeneralName::IPAddress(dns_name) = gn {
                  if *dns_name == local_ip_bytes {
                      ip_found = true;
                      break;
                  }
              }
          }
      }  
    }

    if !ip_found {
        delete_certificate()?;
        generate_certificate()?;
    }

    Ok(())
}

fn write_out_instructions() {
    // Get full path to the certificate directory
    let certificate_dir = get_certificate_dir();

    println!("*******************************************************************************");
    println!("To install the generated self-signed SSL certificate, follow these steps:");
    println!("1. Navigate to {:?} directory", certificate_dir.to_str().unwrap());
    println!("2. Depending on your operating system, the installation process will vary:");
    println!("   - For macOS:");
    println!("     a. Open Keychain Access and select the 'System' keychain.");
    println!("     b. Drag the cert.pem file into Keychain Access.");
    println!("     c. Double-click the certificate, expand the 'Trust' section, and set 'When using this certificate' to 'Always Trust'.");
    println!("   - For Windows:");
    println!("     a. Double-click the cert.pem file.");
    println!("     b. Select 'Install Certificate...', choose the 'Local Machine' store location, and place it in the 'Trusted Root Certification Authorities' store.");
    println!("   - For Linux:");
    println!("     a. The process varies by distribution and browser. For most browsers, importing the certificate through the browser's settings by navigating to 'Privacy and Security' -> 'Manage Certificates' should work.");
    println!("3. After installation, you may need to restart your browser or any application that needs to trust the certificate.");
    println!("*******************************************************************************");    
}

pub fn get_certificate_dir() -> PathBuf {
  // Convert the current file path to a PathBuf
  let current_file_path = Path::new(file!());
  // Get the directory containing the current file
  let parent_dir = current_file_path.parent().expect("Failed to get parent directory");
  // Get the grandparent directory
  let server_dir = parent_dir.parent().expect("Failed to get parent directory");
  // Get certificates directory
  let certificate_dir = server_dir.join("certificates");

  certificate_dir
}
