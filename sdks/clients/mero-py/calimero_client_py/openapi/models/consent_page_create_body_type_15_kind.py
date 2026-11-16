from enum import Enum


class ConsentPageCreateBodyType15Kind(str, Enum):
    SETALLAPPS = "setAllApps"

    def __str__(self) -> str:
        return str(self.value)
