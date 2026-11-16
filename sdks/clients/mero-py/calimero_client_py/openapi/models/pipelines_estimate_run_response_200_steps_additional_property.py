from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="PipelinesEstimateRunResponse200StepsAdditionalProperty")


@_attrs_define
class PipelinesEstimateRunResponse200StepsAdditionalProperty:
    """
    Attributes:
        runs (str):
        fuel (str):
        cost (Union[Unset, str]):
    """

    runs: str
    fuel: str
    cost: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        runs = self.runs

        fuel = self.fuel

        cost = self.cost

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "runs": runs,
                "fuel": fuel,
            }
        )
        if cost is not UNSET:
            field_dict["cost"] = cost

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        runs = d.pop("runs")

        fuel = d.pop("fuel")

        cost = d.pop("cost", UNSET)

        pipelines_estimate_run_response_200_steps_additional_property = cls(
            runs=runs,
            fuel=fuel,
            cost=cost,
        )

        pipelines_estimate_run_response_200_steps_additional_property.additional_properties = (
            d
        )
        return pipelines_estimate_run_response_200_steps_additional_property

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
