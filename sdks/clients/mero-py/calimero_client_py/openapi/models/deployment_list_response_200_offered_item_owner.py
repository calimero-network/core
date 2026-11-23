from enum import Enum


class DeploymentListResponse200OfferedItemOwner(str, Enum):
    ACCOUNT = "account"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
