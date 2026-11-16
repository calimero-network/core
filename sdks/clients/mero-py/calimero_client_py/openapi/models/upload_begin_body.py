from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UploadBeginBody")


@_attrs_define
class UploadBeginBody:
    """
    Attributes:
        size (int):
    """

    size: int

    def to_dict(self) -> dict[str, Any]:
        size = self.size

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "size": size,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        size = d.pop("size")

        upload_begin_body = cls(
            size=size,
        )

        return upload_begin_body
