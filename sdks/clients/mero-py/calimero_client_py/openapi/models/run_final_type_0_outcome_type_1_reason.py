from enum import Enum


class RunFinalType0OutcomeType1Reason(str, Enum):
    APP_ERROR = "app-error"
    CANCELLED = "cancelled"
    FUEL = "fuel"
    INTEGRITY = "integrity"
    LOST = "lost"
    MEMORY = "memory"
    NOT_GRANTED = "not-granted"
    QUOTA = "quota"
    REFUSED = "refused"
    TIMEOUT = "timeout"
    TOO_LARGE = "too-large"
    TRAP = "trap"

    def __str__(self) -> str:
        return str(self.value)
