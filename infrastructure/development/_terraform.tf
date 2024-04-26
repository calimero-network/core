terraform {
  backend "gcs" {
    bucket = "calimero-aws-workload-development-terraform-state"
    prefix = "p2p/aws/development"
  }

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }

    google = {
      source  = "hashicorp/google"
      version = ">= 5.0"
    }
  }

  required_version = "~> 1.7.0"
}
