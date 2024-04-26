module "role_relay_server" {
  source  = "terraform-aws-modules/iam/aws//modules/iam-assumable-role"
  version = "v5.39.0"
  providers = {
    aws = aws.euc1
  }

  # https://aws.amazon.com/blogs/security/announcing-an-update-to-iam-role-trust-policy-behavior/
  allow_self_assume_role  = true
  create_role             = true
  create_instance_profile = true
  role_name               = "relay_server"
  role_requires_mfa       = false

  custom_role_policy_arns = [
    "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
  ]
  trusted_role_services = [
    "ec2.amazonaws.com"
  ]
}
