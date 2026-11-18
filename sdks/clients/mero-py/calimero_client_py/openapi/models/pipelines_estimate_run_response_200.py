from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.pipelines_estimate_run_response_200_steps import (
        PipelinesEstimateRunResponse200Steps,
    )


T = TypeVar("T", bound="PipelinesEstimateRunResponse200")


@_attrs_define
class PipelinesEstimateRunResponse200:
    """
    Attributes:
        steps (PipelinesEstimateRunResponse200Steps):
        fuel (str):
        cost (Union[Unset, str]):
        price_version (Union[Unset, int]):
    """

    steps: "PipelinesEstimateRunResponse200Steps"
    fuel: str
    cost: Union[Unset, str] = UNSET
    price_version: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pipelines_estimate_run_response_200_steps import (
            PipelinesEstimateRunResponse200Steps,
        )

        steps = self.steps.to_dict()

        fuel = self.fuel

        cost = self.cost

        price_version = self.price_version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "steps": steps,
                "fuel": fuel,
            }
        )
        if cost is not UNSET:
            field_dict["cost"] = cost
        if price_version is not UNSET:
            field_dict["priceVersion"] = price_version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pipelines_estimate_run_response_200_steps import (
            PipelinesEstimateRunResponse200Steps,
        )

        d = dict(src_dict)
        steps = PipelinesEstimateRunResponse200Steps.from_dict(d.pop("steps"))

        fuel = d.pop("fuel")

        cost = d.pop("cost", UNSET)

        price_version = d.pop("priceVersion", UNSET)

        pipelines_estimate_run_response_200 = cls(
            steps=steps,
            fuel=fuel,
            cost=cost,
            price_version=price_version,
        )

        pipelines_estimate_run_response_200.additional_properties = d
        return pipelines_estimate_run_response_200

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
