from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ExportsExportBody")


@_attrs_define
class ExportsExportBody:
    """
    Attributes:
        recipient (str):
    """

    recipient: str

    def to_dict(self) -> dict[str, Any]:
        recipient = self.recipient

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "recipient": recipient,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        recipient = d.pop("recipient")

        exports_export_body = cls(
            recipient=recipient,
        )

        return exports_export_body
