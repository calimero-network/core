from enum import Enum


class UpgradeClass(str, Enum):
    GREEN = "green"
    RED = "red"

    def __str__(self) -> str:
        return str(self.value)
