from enum import Enum


class GovernanceDeployments(str, Enum):
    GRANTED = "granted"
    OPEN = "open"

    def __str__(self) -> str:
        return str(self.value)
