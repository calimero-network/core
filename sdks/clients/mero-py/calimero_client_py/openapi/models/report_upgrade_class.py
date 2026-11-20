from enum import Enum


class ReportUpgradeClass(str, Enum):
    GREEN = "green"
    RED = "red"

    def __str__(self) -> str:
        return str(self.value)
