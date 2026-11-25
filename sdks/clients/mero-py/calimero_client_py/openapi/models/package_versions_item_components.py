from enum import Enum


class PackageVersionsItemComponents(str, Enum):
    APP = "app"
    COMPUTEONLY = "computeOnly"

    def __str__(self) -> str:
        return str(self.value)
