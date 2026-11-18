from enum import Enum


class DeployerListResponse200Deployments(str, Enum):
    GRANTED = "granted"
    OPEN = "open"

    def __str__(self) -> str:
        return str(self.value)
