locals {
  aws_account_id = "938392550440"
  aws_profile    = "calimero-development"
}

provider "aws" {
  alias               = "euc1"
  region              = "eu-central-1"
  allowed_account_ids = [local.aws_account_id]
  profile             = local.aws_profile

  default_tags {
    tags = {
      ManagedBy = "Terraform"
      Stack     = "p2p/aws/development"
    }
  }
}

provider "google" {
  project = "calimero-development"
}
