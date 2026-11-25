from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="AppAbi")


@_attrs_define
class AppAbi:
    """`GET /apps/{app}/abi`: the bundle the app group runs and the ABI it embeds (`null`: none).

    Attributes:
        bundle (str):
        abi (Union[Any, None]):
    """

    bundle: str
    abi: Union[Any, None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        bundle = self.bundle

        abi: Union[Any, None]
        abi = self.abi

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "bundle": bundle,
                "abi": abi,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        bundle = d.pop("bundle")

        def _parse_abi(data: object) -> Union[Any, None]:
            if data is None:
                return data
            return cast(Union[Any, None], data)

        abi = _parse_abi(d.pop("abi"))

        app_abi = cls(
            bundle=bundle,
            abi=abi,
        )

        app_abi.additional_properties = d
        return app_abi

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
