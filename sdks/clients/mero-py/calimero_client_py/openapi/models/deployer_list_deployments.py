from enum import Enum


class DeployerListDeployments(str, Enum):
    GRANTED = "granted"
    OPEN = "open"

    def __str__(self) -> str:
        return str(self.value)
