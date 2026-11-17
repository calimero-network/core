from enum import Enum


class ConsentPageCreateBodyType16Kind(str, Enum):
    SETCAPABILITIES = "setCapabilities"

    def __str__(self) -> str:
        return str(self.value)
