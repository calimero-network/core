from enum import Enum


class ReportAppUpgradesHeldBy(str, Enum):
    OPERATOR = "operator"
    OWNER = "owner"
    PUBLISHER = "publisher"

    def __str__(self) -> str:
        return str(self.value)
