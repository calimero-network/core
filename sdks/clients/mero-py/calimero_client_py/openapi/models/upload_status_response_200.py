from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UploadStatusResponse200")


@_attrs_define
class UploadStatusResponse200:
    """
    Attributes:
        upload_id (str):
        part_bytes (int):
        parts (int):
        expires_at (int):
        size (int):
        next_ (int):
    """

    upload_id: str
    part_bytes: int
    parts: int
    expires_at: int
    size: int
    next_: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        upload_id = self.upload_id

        part_bytes = self.part_bytes

        parts = self.parts

        expires_at = self.expires_at

        size = self.size

        next_ = self.next_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "uploadId": upload_id,
                "partBytes": part_bytes,
                "parts": parts,
                "expiresAt": expires_at,
                "size": size,
                "next": next_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        upload_id = d.pop("uploadId")

        part_bytes = d.pop("partBytes")

        parts = d.pop("parts")

        expires_at = d.pop("expiresAt")

        size = d.pop("size")

        next_ = d.pop("next")

        upload_status_response_200 = cls(
            upload_id=upload_id,
            part_bytes=part_bytes,
            parts=parts,
            expires_at=expires_at,
            size=size,
            next_=next_,
        )

        upload_status_response_200.additional_properties = d
        return upload_status_response_200

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
