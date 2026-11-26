from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.served_origins_origins_item import ServedOriginsOriginsItem


T = TypeVar("T", bound="ServedOrigins")


@_attrs_define
class ServedOrigins:
    """`GET /ops/served-origins`: the origins the operator serves frontends on, each with its host, its package's label and
    when it was added (RFC 3339).

        Attributes:
            fresh (bool):
            origins (list['ServedOriginsOriginsItem']):
    """

    fresh: bool
    origins: list["ServedOriginsOriginsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.served_origins_origins_item import ServedOriginsOriginsItem

        fresh = self.fresh

        origins = []
        for origins_item_data in self.origins:
            origins_item = origins_item_data.to_dict()
            origins.append(origins_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fresh": fresh,
                "origins": origins,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.served_origins_origins_item import ServedOriginsOriginsItem

        d = dict(src_dict)
        fresh = d.pop("fresh")

        origins = []
        _origins = d.pop("origins")
        for origins_item_data in _origins:
            origins_item = ServedOriginsOriginsItem.from_dict(origins_item_data)

            origins.append(origins_item)

        served_origins = cls(
            fresh=fresh,
            origins=origins,
        )

        served_origins.additional_properties = d
        return served_origins

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
