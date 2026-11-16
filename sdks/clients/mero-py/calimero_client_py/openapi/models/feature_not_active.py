from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="FeatureNotActive")


@_attrs_define
class FeatureNotActive:
    """`details` of a `409 FEATURE_NOT_ACTIVE`: the feature the request needs, the cluster version it ships in (`needs`)
    and the version active now (`null`: none logged, and the nodes could not say).

        Attributes:
            feature (str):
            needs (str):
            active (Union[None, str]):
    """

    feature: str
    needs: str
    active: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        feature = self.feature

        needs = self.needs

        active: Union[None, str]
        active = self.active

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "feature": feature,
                "needs": needs,
                "active": active,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        feature = d.pop("feature")

        needs = d.pop("needs")

        def _parse_active(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        active = _parse_active(d.pop("active"))

        feature_not_active = cls(
            feature=feature,
            needs=needs,
            active=active,
        )

        feature_not_active.additional_properties = d
        return feature_not_active

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
