from enum import Enum


class RunQueueReason(str, Enum):
    NO_CAPACITY = "NO_CAPACITY"
    PAYER_AT_FUEL_QUOTA = "PAYER_AT_FUEL_QUOTA"
    PAYER_AT_RUN_QUOTA = "PAYER_AT_RUN_QUOTA"
    POOL_FULL = "POOL_FULL"

    def __str__(self) -> str:
        return str(self.value)
