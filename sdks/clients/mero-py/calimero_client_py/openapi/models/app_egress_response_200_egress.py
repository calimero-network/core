from enum import Enum


class AppEgressResponse200Egress(str, Enum):
    OFF = "off"
    ON = "on"

    def __str__(self) -> str:
        return str(self.value)
