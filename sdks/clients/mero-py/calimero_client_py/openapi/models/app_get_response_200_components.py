from enum import Enum


class AppGetResponse200Components(str, Enum):
    COMPUTEONLY = "computeOnly"

    def __str__(self) -> str:
        return str(self.value)
