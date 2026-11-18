from enum import Enum


class DeploymentGetResponse200OwnerKind(str, Enum):
    ACCOUNT = "account"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
