from enum import Enum


class OutboundSelectionType0(str, Enum):
    MANIFEST = "manifest"

    def __str__(self) -> str:
        return str(self.value)
