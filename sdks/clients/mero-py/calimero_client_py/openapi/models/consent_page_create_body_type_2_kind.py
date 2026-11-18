from enum import Enum


class ConsentPageCreateBodyType2Kind(str, Enum):
    UPGRADEUNIT = "upgradeUnit"

    def __str__(self) -> str:
        return str(self.value)
