from enum import Enum


class ReportUpgradeActorKind(str, Enum):
    MEMBER = "member"
    OPERATOR = "operator"
    OWNER = "owner"

    def __str__(self) -> str:
        return str(self.value)
