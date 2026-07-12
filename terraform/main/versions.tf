# Main stack: the service itself (storage, ingest, aggregation,
# distribution, observability). Backs onto the bootstrap stack's state
# bucket; see ../README.md for the order of operations.

terraform {
  required_version = ">= 1.10"

  backend "s3" {
    bucket       = "entropiaorme-market-data-tfstate"
    key          = "market-data/main.tfstate"
    region       = "eu-north-1"
    use_lockfile = true
  }

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.80"
    }
    archive = {
      source  = "hashicorp/archive"
      version = "~> 2.6"
    }
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project   = "entropiaorme-market-data"
      ManagedBy = "terraform"
      Stack     = "main"
    }
  }
}

# CloudFront only accepts ACM certificates issued in us-east-1, wherever
# the rest of the stack lives.
provider "aws" {
  alias  = "us_east_1"
  region = "us-east-1"

  default_tags {
    tags = {
      Project   = "entropiaorme-market-data"
      ManagedBy = "terraform"
      Stack     = "main"
    }
  }
}
