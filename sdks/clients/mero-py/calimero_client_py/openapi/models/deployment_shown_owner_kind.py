from enum import Enum


class DeploymentShownOwnerKind(str, Enum):
    ACCOUNT = "account"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
