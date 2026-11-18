from enum import Enum


class AppComponents(str, Enum):
    COMPUTEONLY = "computeOnly"

    def __str__(self) -> str:
        return str(self.value)
