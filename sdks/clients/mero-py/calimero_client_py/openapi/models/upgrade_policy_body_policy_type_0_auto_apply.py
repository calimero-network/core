from enum import Enum


class UpgradePolicyBodyPolicyType0AutoApply(str, Enum):
    NEVER = "never"
    NODESTINATIONS = "noDestinations"

    def __str__(self) -> str:
        return str(self.value)
