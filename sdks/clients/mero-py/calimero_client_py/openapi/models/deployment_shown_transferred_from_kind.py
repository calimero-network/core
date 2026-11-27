from enum import Enum


class DeploymentShownTransferredFromKind(str, Enum):
    ACCOUNT = "account"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
