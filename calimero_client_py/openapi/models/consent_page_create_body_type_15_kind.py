from enum import Enum


class ConsentPageCreateBodyType15Kind(str, Enum):
    SETCAPABILITIES = "setCapabilities"

    def __str__(self) -> str:
        return str(self.value)
