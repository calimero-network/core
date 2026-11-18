from enum import Enum


class PackageVersionsItemSource(str, Enum):
    REGISTRY = "registry"
    UPLOAD = "upload"

    def __str__(self) -> str:
        return str(self.value)
