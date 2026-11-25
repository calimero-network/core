from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.run_estimate_steps import RunEstimateSteps


T = TypeVar("T", bound="RunEstimate")


@_attrs_define
class RunEstimate:
    """What `POST /apps/{app}/pipelines/{name}/estimate` answers: per step its runs and fuel, the whole run's fuel, and its
    cost and the price table's version when the cluster has one; every figure a decimal string.

        Attributes:
            steps (RunEstimateSteps):
            fuel (str):
            cost (Union[Unset, str]):
            price_version (Union[Unset, int]):
    """

    steps: "RunEstimateSteps"
    fuel: str
    cost: Union[Unset, str] = UNSET
    price_version: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.run_estimate_steps import RunEstimateSteps

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
        from ..models.run_estimate_steps import RunEstimateSteps

        d = dict(src_dict)
        steps = RunEstimateSteps.from_dict(d.pop("steps"))

        fuel = d.pop("fuel")

        cost = d.pop("cost", UNSET)

        price_version = d.pop("priceVersion", UNSET)

        run_estimate = cls(
            steps=steps,
            fuel=fuel,
            cost=cost,
            price_version=price_version,
        )

        run_estimate.additional_properties = d
        return run_estimate

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
