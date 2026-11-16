from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="OpsRestoreResponse200Backup")


@_attrs_define
class OpsRestoreResponse200Backup:
    """
    Attributes:
        name (str):
        index (int):
        format_ (int):
        merod (Union[None, str]):
        release (Union[None, str]):
    """

    name: str
    index: int
    format_: int
    merod: Union[None, str]
    release: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        name = self.name

        index = self.index

        format_ = self.format_

        merod: Union[None, str]
        merod = self.merod

        release: Union[None, str]
        release = self.release

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
                "index": index,
                "format": format_,
                "merod": merod,
                "release": release,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        name = d.pop("name")

        index = d.pop("index")

        format_ = d.pop("format")

        def _parse_merod(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        merod = _parse_merod(d.pop("merod"))

        def _parse_release(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        release = _parse_release(d.pop("release"))

        ops_restore_response_200_backup = cls(
            name=name,
            index=index,
            format_=format_,
            merod=merod,
            release=release,
        )

        ops_restore_response_200_backup.additional_properties = d
        return ops_restore_response_200_backup

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
