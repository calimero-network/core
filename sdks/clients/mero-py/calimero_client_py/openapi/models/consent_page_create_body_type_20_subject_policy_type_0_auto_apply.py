from enum import Enum


class ConsentPageCreateBodyType20SubjectPolicyType0AutoApply(str, Enum):
    NEVER = "never"
    NODESTINATIONS = "noDestinations"

    def __str__(self) -> str:
        return str(self.value)
