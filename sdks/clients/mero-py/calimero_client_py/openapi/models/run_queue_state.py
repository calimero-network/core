from enum import Enum


class RunQueueState(str, Enum):
    GRANTED = "granted"
    UNPLACED = "unplaced"
    WAITING = "waiting"

    def __str__(self) -> str:
        return str(self.value)
