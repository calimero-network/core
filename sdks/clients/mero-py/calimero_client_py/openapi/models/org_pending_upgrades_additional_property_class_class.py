from enum import Enum


class OrgPendingUpgradesAdditionalPropertyClassClass(str, Enum):
    GREEN = "green"
    RED = "red"

    def __str__(self) -> str:
        return str(self.value)
