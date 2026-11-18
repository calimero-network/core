from enum import Enum


class GovernanceAppUpgrades(str, Enum):
    OPERATOR = "operator"
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
