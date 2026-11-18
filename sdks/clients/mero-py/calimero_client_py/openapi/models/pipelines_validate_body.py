from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="PipelinesValidateBody")


@_attrs_define
class PipelinesValidateBody:
    """
    Attributes:
        source (str):
        package (Union[Unset, str]):
        version (Union[Unset, str]):
    """

    source: str
    package: Union[Unset, str] = UNSET
    version: Union[Unset, str] = UNSET

    def to_dict(self) -> dict[str, Any]:
        source = self.source

        package = self.package

        version = self.version

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "source": source,
            }
        )
        if package is not UNSET:
            field_dict["package"] = package
        if version is not UNSET:
            field_dict["version"] = version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        source = d.pop("source")

        package = d.pop("package", UNSET)

        version = d.pop("version", UNSET)

        pipelines_validate_body = cls(
            source=source,
            package=package,
            version=version,
        )

        return pipelines_validate_body
