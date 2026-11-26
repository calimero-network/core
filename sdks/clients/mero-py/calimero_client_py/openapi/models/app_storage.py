from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="AppStorage")


@_attrs_define
class AppStorage:
    """`GET /apps/{app}/storage`: keys, bytes, blobs, and when it last committed a write (Unix seconds; `null` before its
    first).

        Attributes:
            keys (int):
            bytes_ (int):
            blobs (int):
            blob_bytes (int):
            last_write_at (Union[None, int]):
    """

    keys: int
    bytes_: int
    blobs: int
    blob_bytes: int
    last_write_at: Union[None, int]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        keys = self.keys

        bytes_ = self.bytes_

        blobs = self.blobs

        blob_bytes = self.blob_bytes

        last_write_at: Union[None, int]
        last_write_at = self.last_write_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "keys": keys,
                "bytes": bytes_,
                "blobs": blobs,
                "blobBytes": blob_bytes,
                "lastWriteAt": last_write_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        keys = d.pop("keys")

        bytes_ = d.pop("bytes")

        blobs = d.pop("blobs")

        blob_bytes = d.pop("blobBytes")

        def _parse_last_write_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        last_write_at = _parse_last_write_at(d.pop("lastWriteAt"))

        app_storage = cls(
            keys=keys,
            bytes_=bytes_,
            blobs=blobs,
            blob_bytes=blob_bytes,
            last_write_at=last_write_at,
        )

        app_storage.additional_properties = d
        return app_storage

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
