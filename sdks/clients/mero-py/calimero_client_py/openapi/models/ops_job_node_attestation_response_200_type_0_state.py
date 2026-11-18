from enum import Enum


class OpsJobNodeAttestationResponse200Type0State(str, Enum):
    ADMITTED = "admitted"

    def __str__(self) -> str:
        return str(self.value)
