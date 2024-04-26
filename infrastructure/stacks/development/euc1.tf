locals {
  euc1_vpc_id           = "vpc-0d18a46035c7e41df"
  euc1_public_subnet_id = "subnet-00fd5b1bf0cf9df6b"
}

data "aws_ami" "euc1_ubuntu" {
  provider = aws.euc1

  most_recent = true
  owners      = ["amazon"]
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-*"]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

module "euc1_sg_relay_server" {
  source  = "terraform-aws-modules/security-group/aws"
  version = "v5.1.2"
  providers = {
    aws = aws.euc1
  }

  name        = "relay-server"
  description = "Allow inbound traffic for the relay server"
  vpc_id      = local.euc1_vpc_id
  ingress_with_cidr_blocks = [
    {
      from_port   = 1234
      to_port     = 1234
      protocol    = "tcp"
      cidr_blocks = "0.0.0.0/0"
      description = "Allow TCP on 1234"
    },
    {
      from_port   = 1234
      to_port     = 1234
      protocol    = "udp"
      cidr_blocks = "0.0.0.0/0"
      description = "Allow UDP on 1234"
    },
    {
      from_port   = -1
      to_port     = -1
      protocol    = "icmp"
      cidr_blocks = "0.0.0.0/0"
      description = "Allow ICMP"
    },
    {
      from_port   = 22
      to_port     = 22
      protocol    = "tcp"
      cidr_blocks = "0.0.0.0/0"
      description = "Allow TCP on 22"
    },
  ]
}

module "euc1_relay_server_1" {
  source  = "terraform-aws-modules/ec2-instance/aws"
  version = "v5.6.1"
  providers = {
    aws = aws.euc1
  }

  name                   = "relay-server-1"
  ami                    = data.aws_ami.euc1_ubuntu.id
  instance_type          = "m5.2xlarge"
  key_name               = "relay-server"
  monitoring             = true
  vpc_security_group_ids = [module.euc1_sg_relay_server.security_group_id]
  subnet_id              = local.euc1_public_subnet_id
}

resource "aws_eip" "euc1_relay_server_1" {
  provider = aws.euc1

  domain   = "vpc"
  instance = module.euc1_relay_server_1.id
}
